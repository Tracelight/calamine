use std::collections::BTreeMap;
use std::io::{Read, Seek};

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;

use super::{get_row_column, XlReader, XlsxError};
use crate::datatype::DataRef;
use crate::utils::{decode_attr_value, unescape_entity_to_buffer};
use crate::Cell;

#[derive(Debug)]
pub(super) struct ExternalRelationship {
    target: String,
}

/// Defined-name metadata cached in an external-link part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalDefinedName {
    /// The case-preserving identifier stored in the link.
    pub name: String,
    /// The formula text stored in the link after XML entity decoding.
    pub refers_to: Option<String>,
    /// The raw OOXML `sheetId`, when the name is sheet-scoped.
    pub sheet_id: Option<u32>,
}

/// Metadata and cached values emitted from an external-link part.
#[derive(Debug, Clone)]
pub enum ExternalLinkItem<'a> {
    /// The relationship target of a supporting workbook.
    Workbook {
        /// The path or URI stored in the external-link relationship.
        target: String,
    },
    /// A sheet name from the supporting workbook, in workbook order.
    SheetName {
        /// The zero-based position in the external workbook's sheet-name list.
        sheet_id: u32,
        /// The case-preserving display name.
        name: String,
    },
    /// Name metadata cached by the dependent workbook.
    DefinedName(ExternalDefinedName),
    /// The start of one cached external sheet.
    SheetData {
        /// The raw OOXML `sheetId` of this cached sheet.
        sheet_id: u32,
        /// Whether the last refresh of this sheet failed.
        refresh_error: bool,
    },
    /// A cached external cell.
    Cell {
        /// The raw OOXML `sheetId` of the cached sheet.
        sheet_id: u32,
        /// The cached value and zero-based position.
        cell: Cell<'a, DataRef<'a>>,
    },
    /// A DDE link, which does not contain an external workbook cache.
    Dde,
    /// An OLE link, which does not contain an external workbook cache.
    Ole,
}

/// Reads an external-link part without materialising its cached cells.
pub struct XlsxExternalLinkReader<'a, RS>
where
    RS: Read + Seek,
{
    xml: XlReader<'a, RS>,
    strings: &'a [String],
    relationships: BTreeMap<Vec<u8>, ExternalRelationship>,
    sheet_name_index: u32,
    sheet_id: Option<u32>,
    done: bool,
    buf: Vec<u8>,
    cell_buf: Vec<u8>,
    value: String,
}

impl<'a, RS> XlsxExternalLinkReader<'a, RS>
where
    RS: Read + Seek,
{
    pub(super) fn new(
        xml: XlReader<'a, RS>,
        strings: &'a [String],
        relationships: BTreeMap<Vec<u8>, ExternalRelationship>,
    ) -> Self {
        Self {
            xml,
            strings,
            relationships,
            sheet_name_index: 0,
            sheet_id: None,
            done: false,
            buf: Vec::with_capacity(1024),
            cell_buf: Vec::with_capacity(1024),
            value: String::new(),
        }
    }

    /// Yields items in XML document order, returning `None` after the part ends.
    pub fn next_item(&mut self) -> Result<Option<ExternalLinkItem<'a>>, XlsxError> {
        if self.done {
            return Ok(None);
        }

        loop {
            self.buf.clear();
            match self.xml.read_event_into(&mut self.buf) {
                Ok(Event::Start(e)) => match e.local_name().as_ref() {
                    b"externalBook" => {
                        let relationship_id =
                            relationship_id(&e)?.ok_or(XlsxError::RelationshipNotFound)?;
                        let target = self
                            .relationships
                            .get(&relationship_id)
                            .ok_or(XlsxError::RelationshipNotFound)?
                            .target
                            .clone();
                        return Ok(Some(ExternalLinkItem::Workbook { target }));
                    }
                    b"sheetName" => {
                        let name = required_attribute(&e, b"val", &self.xml)?;
                        let sheet_id = self.sheet_name_index;
                        self.sheet_name_index += 1;
                        return Ok(Some(ExternalLinkItem::SheetName { sheet_id, name }));
                    }
                    b"definedName" => {
                        let name = required_attribute(&e, b"name", &self.xml)?;
                        let refers_to = optional_attribute(&e, b"refersTo", &self.xml)?;
                        let sheet_id = optional_u32_attribute(&e, b"sheetId", &self.xml)?;
                        return Ok(Some(ExternalLinkItem::DefinedName(ExternalDefinedName {
                            name,
                            refers_to,
                            sheet_id,
                        })));
                    }
                    b"sheetData" => {
                        let sheet_id = required_u32_attribute(&e, b"sheetId", &self.xml)?;
                        self.sheet_id = Some(sheet_id);
                        let refresh_error =
                            optional_bool_attribute(&e, b"refreshError", &self.xml)?
                                .unwrap_or(false);
                        return Ok(Some(ExternalLinkItem::SheetData {
                            sheet_id,
                            refresh_error,
                        }));
                    }
                    b"cell" => {
                        let sheet_id = self
                            .sheet_id
                            .ok_or(XlsxError::Unexpected("cell outside external sheet data"))?;
                        let cell = read_external_cell(
                            &mut self.xml,
                            &e,
                            self.strings,
                            &mut self.cell_buf,
                            &mut self.value,
                        )?;
                        return Ok(Some(ExternalLinkItem::Cell { sheet_id, cell }));
                    }
                    b"ddeLink" => return Ok(Some(ExternalLinkItem::Dde)),
                    b"oleLink" => return Ok(Some(ExternalLinkItem::Ole)),
                    _ => {}
                },
                Ok(Event::End(e)) if e.local_name().as_ref() == b"sheetData" => {
                    self.sheet_id = None;
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == b"externalLink" => {
                    self.done = true;
                    return Ok(None);
                }
                Ok(Event::Eof) => return Err(XlsxError::XmlEof("externalLink")),
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => {}
            }
        }
    }
}

pub(super) fn read_external_relationships<RS>(
    mut xml: XlReader<'_, RS>,
) -> Result<BTreeMap<Vec<u8>, ExternalRelationship>, XlsxError>
where
    RS: Read + Seek,
{
    let mut relationships = BTreeMap::new();
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"Relationship" => {
                let mut id = None;
                let mut target = None;
                for attribute in e.attributes() {
                    let attribute = attribute?;
                    match attribute.key.as_ref() {
                        b"Id" => id = Some(attribute.value.into_owned()),
                        b"Target" => {
                            target =
                                Some(decode_attr_value(&attribute, xml.decoder())?.into_owned())
                        }
                        _ => {}
                    }
                }
                if let (Some(id), Some(target)) = (id, target) {
                    relationships.insert(id, ExternalRelationship { target });
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"Relationships" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("Relationships")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => {}
        }
    }
    Ok(relationships)
}

fn relationship_id(e: &BytesStart<'_>) -> Result<Option<Vec<u8>>, XlsxError> {
    for attribute in e.attributes() {
        let attribute = attribute?;
        if matches!(attribute.key, QName(b"r:id") | QName(b"relationships:id")) {
            return Ok(Some(attribute.value.into_owned()));
        }
    }
    Ok(None)
}

fn required_attribute<RS>(
    e: &BytesStart<'_>,
    name: &[u8],
    xml: &XlReader<'_, RS>,
) -> Result<String, XlsxError>
where
    RS: Read + Seek,
{
    optional_attribute(e, name, xml)?.ok_or(XlsxError::Unexpected("required attribute not found"))
}

fn optional_attribute<RS>(
    e: &BytesStart<'_>,
    name: &[u8],
    xml: &XlReader<'_, RS>,
) -> Result<Option<String>, XlsxError>
where
    RS: Read + Seek,
{
    for attribute in e.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                decode_attr_value(&attribute, xml.decoder())?.into_owned(),
            ));
        }
    }
    Ok(None)
}

fn optional_u32_attribute<RS>(
    e: &BytesStart<'_>,
    name: &[u8],
    xml: &XlReader<'_, RS>,
) -> Result<Option<u32>, XlsxError>
where
    RS: Read + Seek,
{
    for attribute in e.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == name {
            return decode_attr_value(&attribute, xml.decoder())?
                .parse()
                .map(Some)
                .map_err(XlsxError::ParseInt);
        }
    }
    Ok(None)
}

fn required_u32_attribute<RS>(
    e: &BytesStart<'_>,
    name: &[u8],
    xml: &XlReader<'_, RS>,
) -> Result<u32, XlsxError>
where
    RS: Read + Seek,
{
    optional_u32_attribute(e, name, xml)?.ok_or(XlsxError::Unexpected(
        "required integer attribute not found",
    ))
}

fn optional_bool_attribute<RS>(
    e: &BytesStart<'_>,
    name: &[u8],
    xml: &XlReader<'_, RS>,
) -> Result<Option<bool>, XlsxError>
where
    RS: Read + Seek,
{
    for attribute in e.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == name {
            return Ok(Some(matches!(
                decode_attr_value(&attribute, xml.decoder())?.as_ref(),
                "1" | "true"
            )));
        }
    }
    Ok(None)
}

fn read_external_cell<'a, RS>(
    xml: &mut XlReader<'_, RS>,
    e: &BytesStart<'_>,
    strings: &'a [String],
    event_buf: &mut Vec<u8>,
    value: &mut String,
) -> Result<Cell<'a, DataRef<'a>>, XlsxError>
where
    RS: Read + Seek,
{
    let mut reference = None;
    let mut cell_type = None;
    for attribute in e.attributes() {
        match attribute? {
            Attribute {
                key: QName(b"r"),
                value,
            } => reference = Some(value.into_owned()),
            Attribute {
                key: QName(b"t"),
                value,
            } => cell_type = Some(value.into_owned()),
            _ => {}
        }
    }
    let reference = reference.ok_or(XlsxError::Unexpected(
        "external cell reference attribute not found",
    ))?;
    let position = get_row_column(&reference)?;
    let mut data = DataRef::Empty;

    loop {
        event_buf.clear();
        match xml.read_event_into(event_buf) {
            Ok(Event::Start(v)) if v.local_name().as_ref() == b"v" => {
                let closing = v.name().as_ref().to_vec();
                value.clear();
                loop {
                    event_buf.clear();
                    match xml.read_event_into(event_buf)? {
                        Event::Text(text) => value.push_str(&text.xml10_content()?),
                        Event::GeneralRef(entity) => {
                            unescape_entity_to_buffer(&entity, value)?;
                        }
                        Event::End(end) if end.name().as_ref() == closing => break,
                        Event::Eof => return Err(XlsxError::XmlEof("v")),
                        _ => {}
                    }
                }
                data =
                    super::cells_reader::read_v(value, strings, None, cell_type.as_deref(), false)?;
            }
            Ok(Event::End(end)) if end.local_name().as_ref() == b"cell" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("cell")),
            Err(error) => return Err(XlsxError::Xml(error)),
            _ => {}
        }
    }

    Ok(Cell::new(position, data))
}
