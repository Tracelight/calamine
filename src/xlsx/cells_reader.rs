// SPDX-License-Identifier: MIT
//
// Copyright 2016-2025, Johann Tuffe.

use quick_xml::{
    events::{attributes::Attribute, BytesStart, Event},
    name::QName,
};
use std::{
    borrow::{Borrow, Cow},
    collections::HashMap,
    io::{Read, Seek},
};

use super::{
    get_attribute, get_dimension, get_row, get_row_column, parse_color_from_attrs, read_string,
    replace_cell_names, unchecked_attributes, Dimensions, Theme, XlReader,
};
use crate::{
    datatype::{
        CellFormula, CellFull, DataRef, DataTableFormula, DataTableKind, DataTableOrientation,
    },
    formats::{format_excel_f64_ref, CellFormat},
    style::{ColumnWidthRange, FreezePanes, PaneState, RowHeight, SheetFormat, SheetSettings},
    utils::unescape_entity_to_buffer,
    Cell, Style, XlsxError,
};

type FormulaMap = HashMap<(u32, u32), (i64, i64)>;

/// Scratch buffers for reading a cell's `<v>` and `<f>` text, reused across
/// cells. The event buffer is shared because a cell's value and its formula are
/// never read at the same time.
#[derive(Debug, Default)]
struct ValueScratch {
    event_buf: Vec<u8>,
    text: String,
}

#[derive(Debug, Default)]
struct CellAttributes<'a> {
    reference: Option<&'a [u8]>,
    style_id: usize,
    format_id: Option<usize>,
    cell_type: Option<&'a [u8]>,
}

/// Item returned by the XLSX worksheet XML stream.
#[derive(Debug, Clone)]
pub enum WorksheetItem<'a> {
    /// Sheet-level settings from `sheetPr` and `sheetViews`.
    SheetSettings(SheetSettings),
    /// Default sheet formatting from `sheetFormatPr`.
    SheetFormat(SheetFormat),
    /// Column layout range from a `cols` child.
    ColumnWidthRange(ColumnWidthRange),
    /// Row layout from `row` attributes.
    RowHeight(RowHeight),
    /// A cell from `sheetData`.
    Cell(Cell<'a, CellFull<'a>>),
    /// A merged cell region from `mergeCells`.
    MergedRegion(Dimensions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorksheetItemReaderPhase {
    BeforeSheetData,
    InCols,
    InSheetData,
    AfterSheetData,
    InMergeCells,
    Done,
}

/// An xlsx Cell Iterator
pub struct XlsxCellReader<'a, RS>
where
    RS: Read + Seek,
{
    xml: XlReader<'a, RS>,
    strings: &'a [String],
    formats: &'a [CellFormat],
    styles: &'a [Style],
    is_1904: bool,
    dimensions: Dimensions,
    row_index: u32,
    col_index: u32,
    buf: Vec<u8>,
    cell_buf: Vec<u8>,
    value_scratch: ValueScratch,
    formulas: Vec<Option<(String, FormulaMap)>>,
}

/// An xlsx worksheet item iterator.
pub struct XlsxWorksheetItemReader<'a, RS>
where
    RS: Read + Seek,
{
    xml: XlReader<'a, RS>,
    strings: &'a [String],
    formats: &'a [CellFormat],
    styles: &'a [Style],
    theme: Option<&'a Theme>,
    is_1904: bool,
    row_index: u32,
    col_index: u32,
    buf: Vec<u8>,
    cell_buf: Vec<u8>,
    value_scratch: ValueScratch,
    shared_formula_parents: HashMap<usize, (u32, u32)>,
    phase: WorksheetItemReaderPhase,
    sheet_settings: SheetSettings,
}

impl<'a, RS> XlsxCellReader<'a, RS>
where
    RS: Read + Seek,
{
    pub fn new(
        mut xml: XlReader<'a, RS>,
        strings: &'a [String],
        formats: &'a [CellFormat],
        styles: &'a [Style],
        is_1904: bool,
    ) -> Result<Self, XlsxError> {
        let mut dimensions = Dimensions {
            start: (0, 0),
            end: (0, 0),
        };
        let mut buf = Vec::with_capacity(1024);
        let mut sh_type: Option<String> = None;
        'xml: loop {
            buf.clear();
            match xml.read_event_into(&mut buf).map_err(XlsxError::Xml)? {
                Event::Start(ref e) => match e.local_name().as_ref() {
                    b"dimension" => {
                        for a in e.attributes() {
                            if let Attribute {
                                key: QName(b"ref"),
                                value: rdim,
                            } = a.map_err(XlsxError::XmlAttr)?
                            {
                                dimensions = get_dimension(&rdim)?;
                                continue 'xml;
                            }
                        }
                        return Err(XlsxError::UnexpectedNode("dimension"));
                    }
                    b"sheetData" => break,
                    typ => {
                        if sh_type.is_none() {
                            sh_type = Some(xml.decoder().decode(typ)?.to_string());
                        }
                    }
                },
                Event::Eof => {
                    if let Some(typ) = sh_type {
                        return Err(XlsxError::NotAWorksheet(typ));
                    } else {
                        return Err(XlsxError::XmlEof("worksheet"));
                    }
                }
                _ => (),
            }
        }
        Ok(Self {
            xml,
            strings,
            formats,
            styles,
            is_1904,
            dimensions,
            row_index: 0,
            col_index: 0,
            buf: Vec::with_capacity(1024),
            cell_buf: Vec::with_capacity(1024),
            value_scratch: ValueScratch::default(),
            formulas: Vec::with_capacity(1024),
        })
    }

    pub fn dimensions(&self) -> Dimensions {
        self.dimensions
    }

    pub fn next_cell(&mut self) -> Result<Option<Cell<'a, DataRef<'a>>>, XlsxError> {
        loop {
            self.buf.clear();
            match self.xml.read_event_into(&mut self.buf) {
                Ok(Event::Start(row_element)) if row_element.local_name().as_ref() == b"row" => {
                    let attribute = get_attribute(unchecked_attributes(&row_element), QName(b"r"))?;
                    if let Some(range) = attribute {
                        let row = get_row(range)?;
                        self.row_index = row;
                    }
                }
                Ok(Event::End(row_element)) if row_element.local_name().as_ref() == b"row" => {
                    self.row_index += 1;
                    self.col_index = 0;
                }
                Ok(Event::Start(c_element)) if c_element.local_name().as_ref() == b"c" => {
                    let attrs = read_cell_attributes(&c_element)?;
                    let pos = if let Some(range) = attrs.reference {
                        let (row, col) = get_row_column(range)?;
                        self.col_index = col;
                        (row, col)
                    } else {
                        (self.row_index, self.col_index)
                    };
                    let mut value = DataRef::Empty;
                    let mut style = None;

                    if attrs.style_id < self.styles.len() {
                        style = Some(&self.styles[attrs.style_id]);
                    }

                    let cell_format = attrs.format_id.and_then(|id| self.formats.get(id));
                    loop {
                        self.cell_buf.clear();
                        match self.xml.read_event_into(&mut self.cell_buf) {
                            Ok(Event::Start(e)) => {
                                value = read_value(
                                    self.strings,
                                    self.is_1904,
                                    &mut self.xml,
                                    &e,
                                    cell_format,
                                    attrs.cell_type,
                                    &mut self.value_scratch,
                                )?;
                            }
                            Ok(Event::End(e)) if e.local_name().as_ref() == b"c" => break,
                            Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
                            Err(e) => return Err(XlsxError::Xml(e)),
                            _ => (),
                        }
                    }
                    self.col_index += 1;

                    if let Some(cell_style) = style {
                        return Ok(Some(Cell::with_style(pos, value, cell_style)));
                    } else {
                        return Ok(Some(Cell::new(pos, value)));
                    }
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == b"sheetData" => {
                    return Ok(None);
                }
                Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetData")),
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => (),
            }
        }
    }

    pub fn next_formula(&mut self) -> Result<Option<Cell<'a, String>>, XlsxError> {
        loop {
            self.buf.clear();
            match self.xml.read_event_into(&mut self.buf) {
                Ok(Event::Start(row_element)) if row_element.local_name().as_ref() == b"row" => {
                    let attribute = get_attribute(unchecked_attributes(&row_element), QName(b"r"))?;
                    if let Some(range) = attribute {
                        let row = get_row(range)?;
                        self.row_index = row;
                    }
                }
                Ok(Event::End(row_element)) if row_element.local_name().as_ref() == b"row" => {
                    self.row_index += 1;
                    self.col_index = 0;
                }
                Ok(Event::Start(c_element)) if c_element.local_name().as_ref() == b"c" => {
                    let attrs = read_cell_attributes(&c_element)?;
                    let pos = if let Some(range) = attrs.reference {
                        let (row, col) = get_row_column(range)?;
                        self.col_index = col;
                        (row, col)
                    } else {
                        (self.row_index, self.col_index)
                    };
                    let mut value = None;
                    let mut style = None;

                    if attrs.style_id < self.styles.len() {
                        style = Some(&self.styles[attrs.style_id]);
                    }

                    loop {
                        self.cell_buf.clear();
                        match self.xml.read_event_into(&mut self.cell_buf) {
                            Ok(Event::Start(e)) => {
                                let formula = read_formula(
                                    &mut self.xml,
                                    &e,
                                    &mut self.value_scratch.event_buf,
                                )?;
                                if let Some(f) = formula.borrow() {
                                    value = Some(f.clone());
                                }
                                if let Ok(Some(b"shared")) =
                                    get_attribute(unchecked_attributes(&e), QName(b"t"))
                                {
                                    // shared formula
                                    let mut offset_map: HashMap<(u32, u32), (i64, i64)> =
                                        HashMap::new();
                                    // shared index
                                    let shared_index = match get_attribute(
                                        unchecked_attributes(&e),
                                        QName(b"si"),
                                    )? {
                                        Some(res) => match atoi_simd::parse::<usize>(res) {
                                            Ok(res) => res,
                                            Err(_) => {
                                                return Err(XlsxError::Unexpected(
                                                    "si attribute must be a number",
                                                ));
                                            }
                                        },
                                        None => {
                                            return Err(XlsxError::Unexpected(
                                                "si attribute is mandatory if it is shared",
                                            ));
                                        }
                                    };
                                    // shared reference
                                    match get_attribute(unchecked_attributes(&e), QName(b"ref"))? {
                                        Some(res) => {
                                            // original reference formula
                                            let reference = get_dimension(res)?;

                                            for row in reference.start.0..=reference.end.0 {
                                                for column in reference.start.1..=reference.end.1 {
                                                    offset_map.insert(
                                                        (row, column),
                                                        (
                                                            row as i64 - pos.0 as i64,
                                                            column as i64 - pos.1 as i64,
                                                        ),
                                                    );
                                                }
                                            }
                                            if let Some(f) = formula.borrow() {
                                                if self.formulas.len() <= shared_index {
                                                    self.formulas.resize(shared_index + 1, None);
                                                }
                                                self.formulas[shared_index] =
                                                    Some((f.clone(), offset_map));
                                            }
                                            value = formula;
                                        }
                                        None => {
                                            // calculated formula
                                            if let Some(Some((f, offset_map))) =
                                                self.formulas.get(shared_index)
                                            {
                                                if let Some(offset) = offset_map.get(&pos) {
                                                    value = Some(replace_cell_names(f, *offset)?);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(Event::End(e)) if e.local_name().as_ref() == b"c" => break,
                            Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
                            Err(e) => return Err(XlsxError::Xml(e)),
                            _ => (),
                        }
                    }
                    self.col_index += 1;

                    if let Some(cell_style) = style {
                        return Ok(Some(Cell::with_style(
                            pos,
                            value.unwrap_or_default(),
                            cell_style,
                        )));
                    } else {
                        return Ok(Some(Cell::new(pos, value.unwrap_or_default())));
                    }
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == b"sheetData" => {
                    return Ok(None);
                }
                Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetData")),
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => (),
            }
        }
    }

    pub fn next_style(&mut self) -> Result<Option<Cell<'a, &'a Style>>, XlsxError> {
        loop {
            self.buf.clear();
            match self.xml.read_event_into(&mut self.buf) {
                Ok(Event::Start(ref row_element))
                    if row_element.local_name().as_ref() == b"row" =>
                {
                    let attribute = get_attribute(unchecked_attributes(row_element), QName(b"r"))?;
                    if let Some(range) = attribute {
                        let row = get_row(range)?;
                        self.row_index = row;
                    }
                }
                Ok(Event::End(ref row_element)) if row_element.local_name().as_ref() == b"row" => {
                    self.row_index += 1;
                    self.col_index = 0;
                }
                Ok(Event::Start(ref c_element)) if c_element.local_name().as_ref() == b"c" => {
                    let attrs = read_cell_attributes(c_element)?;
                    let pos = if let Some(range) = attrs.reference {
                        let (row, col) = get_row_column(range)?;
                        self.col_index = col;
                        (row, col)
                    } else {
                        (self.row_index, self.col_index)
                    };

                    let style = if attrs.style_id < self.styles.len() {
                        &self.styles[attrs.style_id]
                    } else {
                        // For out-of-bounds style IDs, use the default style at index 0
                        &self.styles[0]
                    };

                    // Skip the cell content since we only care about the style
                    loop {
                        self.cell_buf.clear();
                        match self.xml.read_event_into(&mut self.cell_buf) {
                            Ok(Event::End(ref e)) if e.local_name().as_ref() == b"c" => break,
                            Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
                            Err(e) => return Err(XlsxError::Xml(e)),
                            _ => (),
                        }
                    }
                    self.col_index += 1;
                    return Ok(Some(Cell::new(pos, style)));
                }
                Ok(Event::End(ref e)) if e.local_name().as_ref() == b"sheetData" => {
                    return Ok(None);
                }
                Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetData")),
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => (),
            }
        }
    }
}

impl<'a, RS> XlsxWorksheetItemReader<'a, RS>
where
    RS: Read + Seek,
{
    /// Create a new XLSX worksheet item reader.
    pub fn new(
        mut xml: XlReader<'a, RS>,
        strings: &'a [String],
        formats: &'a [CellFormat],
        styles: &'a [Style],
        theme: Option<&'a Theme>,
        is_1904: bool,
    ) -> Result<Self, XlsxError> {
        let mut buf = Vec::with_capacity(1024);
        loop {
            buf.clear();
            match xml.read_event_into(&mut buf).map_err(XlsxError::Xml)? {
                Event::Start(e) if e.local_name().as_ref() == b"worksheet" => break,
                Event::Start(e) => {
                    return Err(XlsxError::NotAWorksheet(
                        xml.decoder().decode(e.local_name().as_ref())?.to_string(),
                    ));
                }
                Event::Eof => return Err(XlsxError::XmlEof("worksheet")),
                _ => (),
            }
        }

        Ok(Self {
            xml,
            strings,
            formats,
            styles,
            theme,
            is_1904,
            row_index: 0,
            col_index: 0,
            buf,
            cell_buf: Vec::with_capacity(1024),
            value_scratch: ValueScratch::default(),
            shared_formula_parents: HashMap::new(),
            phase: WorksheetItemReaderPhase::BeforeSheetData,
            sheet_settings: SheetSettings::default(),
        })
    }

    /// Return the next worksheet item from the XLSX XML stream.
    pub fn next_item(&mut self) -> Result<Option<WorksheetItem<'a>>, XlsxError> {
        if self.phase == WorksheetItemReaderPhase::Done {
            return Ok(None);
        }

        loop {
            self.buf.clear();
            match self.xml.read_event_into(&mut self.buf) {
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::BeforeSheetData
                        && e.local_name().as_ref() == b"dimension" => {}
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::BeforeSheetData
                        && e.local_name().as_ref() == b"sheetPr" =>
                {
                    read_sheet_pr(
                        &mut self.xml,
                        &mut self.cell_buf,
                        &mut self.sheet_settings,
                        self.theme,
                    )?;
                }
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::BeforeSheetData
                        && e.local_name().as_ref() == b"sheetViews" =>
                {
                    read_sheet_views(&mut self.xml, &mut self.cell_buf, &mut self.sheet_settings)?;
                }
                Ok(Event::Start(e) | Event::Empty(e))
                    if self.phase == WorksheetItemReaderPhase::BeforeSheetData
                        && e.local_name().as_ref() == b"sheetFormatPr" =>
                {
                    if let Some(format) = read_sheet_format(&self.xml, &e)? {
                        return Ok(Some(WorksheetItem::SheetFormat(format)));
                    }
                }
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::BeforeSheetData
                        && e.local_name().as_ref() == b"cols" =>
                {
                    self.phase = WorksheetItemReaderPhase::InCols;
                }
                Ok(Event::Start(e) | Event::Empty(e))
                    if self.phase == WorksheetItemReaderPhase::InCols
                        && e.local_name().as_ref() == b"col" =>
                {
                    if let Some(column_width) = read_column_width_range(&self.xml, &e)? {
                        return Ok(Some(WorksheetItem::ColumnWidthRange(column_width)));
                    }
                }
                Ok(Event::End(e))
                    if self.phase == WorksheetItemReaderPhase::InCols
                        && e.local_name().as_ref() == b"cols" =>
                {
                    self.phase = WorksheetItemReaderPhase::BeforeSheetData;
                }
                Ok(Event::Eof) if self.phase == WorksheetItemReaderPhase::InCols => {
                    return Err(XlsxError::XmlEof("cols"));
                }
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::BeforeSheetData
                        && e.local_name().as_ref() == b"sheetData" =>
                {
                    self.phase = WorksheetItemReaderPhase::InSheetData;
                    return Ok(Some(WorksheetItem::SheetSettings(
                        self.sheet_settings.clone(),
                    )));
                }
                Ok(Event::Start(row_element))
                    if self.phase == WorksheetItemReaderPhase::InSheetData
                        && row_element.local_name().as_ref() == b"row" =>
                {
                    let attribute = get_attribute(unchecked_attributes(&row_element), QName(b"r"))?;
                    if let Some(range) = attribute {
                        let row = get_row(range)?;
                        self.row_index = row;
                    }
                    if let Some(row_height) = read_row_height(&self.xml, &row_element)? {
                        return Ok(Some(WorksheetItem::RowHeight(row_height)));
                    }
                }
                Ok(Event::End(row_element))
                    if self.phase == WorksheetItemReaderPhase::InSheetData
                        && row_element.local_name().as_ref() == b"row" =>
                {
                    self.row_index += 1;
                    self.col_index = 0;
                }
                Ok(Event::Start(c_element))
                    if self.phase == WorksheetItemReaderPhase::InSheetData
                        && c_element.local_name().as_ref() == b"c" =>
                {
                    let attrs = read_cell_attributes(&c_element)?;
                    let pos = if let Some(range) = attrs.reference {
                        let (row, col) = get_row_column(range)?;
                        self.col_index = col;
                        (row, col)
                    } else {
                        (self.row_index, self.col_index)
                    };
                    let cell = read_cell_full(
                        &mut self.xml,
                        &mut self.cell_buf,
                        self.strings,
                        self.formats,
                        self.styles,
                        self.is_1904,
                        &mut self.shared_formula_parents,
                        &attrs,
                        pos,
                        &mut self.value_scratch,
                    )?;
                    self.col_index += 1;
                    return Ok(Some(WorksheetItem::Cell(cell)));
                }
                Ok(Event::End(e))
                    if self.phase == WorksheetItemReaderPhase::InSheetData
                        && e.local_name().as_ref() == b"sheetData" =>
                {
                    self.phase = WorksheetItemReaderPhase::AfterSheetData;
                }
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::AfterSheetData
                        && e.local_name().as_ref() == b"mergeCells" =>
                {
                    self.phase = WorksheetItemReaderPhase::InMergeCells;
                }
                Ok(Event::Start(e))
                    if self.phase == WorksheetItemReaderPhase::InMergeCells
                        && e.local_name().as_ref() == b"mergeCell" =>
                {
                    if let Some(attr) = get_attribute(e.attributes(), QName(b"ref"))? {
                        return Ok(Some(WorksheetItem::MergedRegion(get_dimension(attr)?)));
                    }
                }
                Ok(Event::End(e))
                    if self.phase == WorksheetItemReaderPhase::InMergeCells
                        && e.local_name().as_ref() == b"mergeCells" =>
                {
                    self.phase = WorksheetItemReaderPhase::AfterSheetData;
                }
                Ok(Event::Eof) => {
                    self.phase = WorksheetItemReaderPhase::Done;
                    return Ok(None);
                }
                Err(e) => return Err(XlsxError::Xml(e)),
                _ => (),
            }
        }
    }
}

fn read_sheet_pr<RS>(
    xml: &mut XlReader<'_, RS>,
    buf: &mut Vec<u8>,
    sheet_settings: &mut SheetSettings,
    theme: Option<&Theme>,
) -> Result<(), XlsxError>
where
    RS: Read + Seek,
{
    loop {
        buf.clear();
        match xml.read_event_into(buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"tabColor" => {
                sheet_settings.tab_color = parse_color_from_attrs(&e.attributes(), theme);
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"sheetPr" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetPr")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
    }
    Ok(())
}

fn read_sheet_views<RS>(
    xml: &mut XlReader<'_, RS>,
    buf: &mut Vec<u8>,
    sheet_settings: &mut SheetSettings,
) -> Result<(), XlsxError>
where
    RS: Read + Seek,
{
    loop {
        buf.clear();
        match xml.read_event_into(buf) {
            Ok(Event::Empty(e)) if e.local_name().as_ref() == b"sheetView" => {
                read_sheet_view_settings(&e, sheet_settings)?;
            }
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"sheetView" => {
                read_sheet_view_settings(&e, sheet_settings)?;
                loop {
                    buf.clear();
                    match xml.read_event_into(buf) {
                        Ok(Event::Start(pane_e) | Event::Empty(pane_e))
                            if pane_e.local_name().as_ref() == b"pane" =>
                        {
                            sheet_settings.freeze_panes = Some(read_freeze_panes(xml, &pane_e)?);
                        }
                        Ok(Event::End(end_e)) if end_e.local_name().as_ref() == b"sheetView" => {
                            break;
                        }
                        Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetView")),
                        Err(e) => return Err(XlsxError::Xml(e)),
                        _ => (),
                    }
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"sheetViews" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("sheetViews")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
    }
    Ok(())
}

fn read_sheet_view_settings(
    e: &BytesStart<'_>,
    sheet_settings: &mut SheetSettings,
) -> Result<(), XlsxError> {
    for attr in e.attributes() {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        if attr.key.as_ref() == b"showGridLines" {
            sheet_settings.show_grid_lines =
                attr.value.as_ref() != b"0" && attr.value.as_ref() != b"false";
        }
    }
    Ok(())
}

fn read_freeze_panes<RS>(
    xml: &XlReader<'_, RS>,
    e: &BytesStart<'_>,
) -> Result<FreezePanes, XlsxError>
where
    RS: Read + Seek,
{
    let mut freeze = FreezePanes::default();
    for attr in e.attributes() {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        match attr.key.as_ref() {
            b"xSplit" => {
                if let Ok(s) = xml.decoder().decode(&attr.value) {
                    freeze.x_split = s.parse().unwrap_or(0.0);
                }
            }
            b"ySplit" => {
                if let Ok(s) = xml.decoder().decode(&attr.value) {
                    freeze.y_split = s.parse().unwrap_or(0.0);
                }
            }
            b"topLeftCell" => {
                if let Ok(s) = xml.decoder().decode(&attr.value) {
                    freeze.top_left_cell = Some(s.to_string());
                }
            }
            b"state" => {
                if let Ok(s) = xml.decoder().decode(&attr.value) {
                    freeze.state = match s.as_ref() {
                        "frozen" => PaneState::Frozen,
                        "frozenSplit" => PaneState::FrozenSplit,
                        "split" => PaneState::Split,
                        _ => PaneState::Frozen,
                    };
                }
            }
            _ => (),
        }
    }
    Ok(freeze)
}

fn read_sheet_format<RS>(
    xml: &XlReader<'_, RS>,
    e: &BytesStart<'_>,
) -> Result<Option<SheetFormat>, XlsxError>
where
    RS: Read + Seek,
{
    let mut format = SheetFormat::default();
    let mut has_attr = false;
    for attr in e.attributes() {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        match attr.key.as_ref() {
            b"baseColWidth" => {
                if let Ok(width_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(width) = width_str.parse::<u32>() {
                        format.base_column_width = Some(width);
                        has_attr = true;
                    }
                }
            }
            b"defaultColWidth" => {
                if let Ok(width_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(width) = width_str.parse::<f64>() {
                        format.default_column_width = Some(width);
                        has_attr = true;
                    }
                }
            }
            b"defaultRowHeight" => {
                if let Ok(height_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(height) = height_str.parse::<f64>() {
                        format.default_row_height = Some(height);
                        has_attr = true;
                    }
                }
            }
            _ => (),
        }
    }
    Ok(has_attr.then_some(format))
}

fn read_column_width_range<RS>(
    xml: &XlReader<'_, RS>,
    col_e: &BytesStart<'_>,
) -> Result<Option<ColumnWidthRange>, XlsxError>
where
    RS: Read + Seek,
{
    let mut col_info = None;
    let mut max_col: Option<u32> = None;
    let mut width = 0.0;
    let mut custom_width = false;
    let mut hidden = false;
    let mut best_fit = false;
    let mut outline_level: u8 = 0;
    let mut collapsed = false;
    let mut style = None;

    for attr in unchecked_attributes(col_e) {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        match attr.key.as_ref() {
            b"min" => {
                if let Ok(min_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(min_val) = min_str.parse::<u32>() {
                        col_info = min_val.checked_sub(1);
                    }
                }
            }
            b"max" => {
                if let Ok(max_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(max_val) = max_str.parse::<u32>() {
                        max_col = max_val.checked_sub(1);
                    }
                }
            }
            b"width" => {
                if let Ok(width_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(w) = width_str.parse::<f64>() {
                        width = w;
                    }
                }
            }
            b"customWidth" => custom_width = attr.value.as_ref() != b"0",
            b"hidden" => hidden = attr.value.as_ref() != b"0",
            b"bestFit" => best_fit = attr.value.as_ref() != b"0",
            b"outlineLevel" => {
                if let Ok(level_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(level) = level_str.parse::<u8>() {
                        outline_level = level.min(7);
                    }
                }
            }
            b"collapsed" => collapsed = attr.value.as_ref() != b"0",
            b"style" => {
                if let Ok(value) = atoi_simd::parse::<u32>(attr.value.as_ref()) {
                    style = Some(value);
                }
            }
            _ => (),
        }
    }

    let Some(min) = col_info else {
        return Ok(None);
    };

    if min > 16383 {
        return Ok(None);
    }

    let max = max_col.unwrap_or(min).max(min).min(16383);
    Ok(Some(ColumnWidthRange {
        first_column: min,
        last_column: max,
        width,
        custom_width,
        hidden,
        best_fit,
        outline_level,
        collapsed,
        style,
    }))
}

fn read_row_height<RS>(
    xml: &XlReader<'_, RS>,
    row_e: &BytesStart<'_>,
) -> Result<Option<RowHeight>, XlsxError>
where
    RS: Read + Seek,
{
    let mut row_num = None;
    let mut height = 0.0;
    let mut custom_height = false;
    let mut hidden = false;
    let mut thick_top = false;
    let mut thick_bottom = false;
    let mut outline_level: u8 = 0;
    let mut collapsed = false;
    let mut style = None;

    for attr in unchecked_attributes(row_e) {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        match attr.key.as_ref() {
            b"r" => {
                if let Ok(row_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(r) = row_str.parse::<u32>() {
                        row_num = r.checked_sub(1);
                    }
                }
            }
            b"ht" => {
                if let Ok(height_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(h) = height_str.parse::<f64>() {
                        height = h;
                    }
                }
            }
            b"customHeight" => custom_height = attr.value.as_ref() != b"0",
            b"hidden" => hidden = attr.value.as_ref() != b"0",
            b"thickTop" => thick_top = attr.value.as_ref() != b"0",
            b"thickBot" => thick_bottom = attr.value.as_ref() != b"0",
            b"outlineLevel" => {
                if let Ok(level_str) = xml.decoder().decode(&attr.value) {
                    if let Ok(l) = level_str.parse::<u8>() {
                        outline_level = l.min(7);
                    }
                }
            }
            b"collapsed" => collapsed = attr.value.as_ref() != b"0",
            b"s" => {
                if let Ok(value) = atoi_simd::parse::<u32>(attr.value.as_ref()) {
                    style = Some(value);
                }
            }
            _ => (),
        }
    }

    let Some(row) = row_num else {
        return Ok(None);
    };

    if custom_height
        || hidden
        || thick_top
        || thick_bottom
        || height > 0.0
        || outline_level > 0
        || collapsed
        || style.is_some()
    {
        Ok(Some(RowHeight {
            row,
            height,
            custom_height,
            hidden,
            thick_top,
            thick_bottom,
            outline_level,
            collapsed,
            style,
        }))
    } else {
        Ok(None)
    }
}

fn read_cell_attributes<'a>(e: &'a BytesStart<'a>) -> Result<CellAttributes<'a>, XlsxError> {
    let mut attrs = CellAttributes::default();
    for attr in unchecked_attributes(e) {
        match attr {
            Ok(Attribute {
                key,
                value: Cow::Borrowed(value),
            }) => match key.as_ref() {
                b"r" => attrs.reference = Some(value),
                b"s" => {
                    let id = atoi_simd::parse::<usize>(value).unwrap_or(0);
                    attrs.style_id = id;
                    attrs.format_id = Some(id);
                }
                b"t" => attrs.cell_type = Some(value),
                _ => (),
            },
            Err(e) => return Err(XlsxError::XmlAttr(e)),
            _ => (),
        }
    }
    Ok(attrs)
}

#[allow(clippy::too_many_arguments)]
fn read_cell_full<'a, RS>(
    xml: &mut XlReader<'_, RS>,
    cell_buf: &mut Vec<u8>,
    strings: &'a [String],
    formats: &'a [CellFormat],
    styles: &'a [Style],
    is_1904: bool,
    shared_formula_parents: &mut HashMap<usize, (u32, u32)>,
    attrs: &CellAttributes<'_>,
    pos: (u32, u32),
    scratch: &mut ValueScratch,
) -> Result<Cell<'a, CellFull<'a>>, XlsxError>
where
    RS: Read + Seek,
{
    let mut value = DataRef::Empty;
    let mut formula = None;
    let mut style = None;

    if attrs.style_id < styles.len() {
        style = Some(&styles[attrs.style_id]);
    }

    let cell_format = attrs.format_id.and_then(|id| formats.get(id));
    loop {
        cell_buf.clear();
        match xml.read_event_into(cell_buf) {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"v" | b"is" => {
                    value = read_value(
                        strings,
                        is_1904,
                        xml,
                        &e,
                        cell_format,
                        attrs.cell_type,
                        scratch,
                    )?;
                }
                b"f" => {
                    formula = read_cell_full_formula(
                        xml,
                        shared_formula_parents,
                        pos,
                        &e,
                        &mut scratch.event_buf,
                    )?;
                }
                _ => return Err(XlsxError::UnexpectedNode("v, f, or is")),
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"c" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("c")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
    }

    let cell = CellFull { value, formula };
    if let Some(cell_style) = style {
        Ok(Cell::with_style(pos, cell, cell_style))
    } else {
        Ok(Cell::new(pos, cell))
    }
}

fn read_value<'s, RS>(
    strings: &'s [String],
    is_1904: bool,
    xml: &mut XlReader<'_, RS>,
    e: &BytesStart<'_>,
    cell_format: Option<&CellFormat>,
    cell_type: Option<&[u8]>,
    scratch: &mut ValueScratch,
) -> Result<DataRef<'s>, XlsxError>
where
    RS: Read + Seek,
{
    Ok(match e.local_name().as_ref() {
        b"is" => {
            // inlineStr
            read_string(xml, e.name())?.map_or(DataRef::Empty, DataRef::String)
        }
        b"v" => {
            // value
            scratch.text.clear();
            loop {
                scratch.event_buf.clear();
                match xml.read_event_into(&mut scratch.event_buf)? {
                    Event::Text(t) => scratch.text.push_str(&t.xml10_content()?),
                    Event::GeneralRef(e) => unescape_entity_to_buffer(&e, &mut scratch.text)?,
                    Event::End(end) if end.name() == e.name() => break,
                    Event::Eof => return Err(XlsxError::XmlEof("v")),
                    _ => (),
                }
            }
            read_v(&mut scratch.text, strings, cell_format, cell_type, is_1904)?
        }
        b"f" => {
            scratch.event_buf.clear();
            xml.read_to_end_into(e.name(), &mut scratch.event_buf)?;
            DataRef::Empty
        }
        _n => return Err(XlsxError::UnexpectedNode("v, f, or is")),
    })
}

fn read_cell_full_formula<RS>(
    xml: &mut XlReader<'_, RS>,
    shared_formula_parents: &mut HashMap<usize, (u32, u32)>,
    pos: (u32, u32),
    e: &BytesStart<'_>,
    event_buf: &mut Vec<u8>,
) -> Result<Option<CellFormula>, XlsxError>
where
    RS: Read + Seek,
{
    let formula_type = get_attribute(unchecked_attributes(e), QName(b"t"))?;

    if let Some(b"dataTable") = formula_type {
        let data_table = parse_data_table_formula(e)?;
        // The dataTable <f> has no inner text; consume the (expand_empty_elements)
        // body + matching </f> so the cell loop stays aligned, same as the Text path.
        read_formula(xml, e, event_buf)?;
        return Ok(Some(CellFormula::DataTable(Box::new(data_table))));
    }

    let is_shared = matches!(formula_type, Some(b"shared"));

    if !is_shared {
        let formula = read_formula(xml, e, event_buf)?.unwrap_or_default();
        return Ok(Some(CellFormula::Text(formula)));
    }

    let shared_index = match get_attribute(unchecked_attributes(e), QName(b"si"))? {
        Some(res) => match atoi_simd::parse::<usize>(res) {
            Ok(res) => res,
            Err(_) => return Err(XlsxError::Unexpected("si attribute must be a number")),
        },
        None => {
            return Err(XlsxError::Unexpected(
                "si attribute is mandatory if it is shared",
            ));
        }
    };

    let formula = read_formula(xml, e, event_buf)?.unwrap_or_default();
    if !formula.is_empty() {
        shared_formula_parents.insert(shared_index, pos);
        return Ok(Some(CellFormula::Text(formula)));
    }

    let parent = shared_formula_parents
        .get(&shared_index)
        .copied()
        .ok_or(XlsxError::Unexpected("shared formula parent not found"))?;

    Ok(Some(CellFormula::Shared { parent }))
}

/// Decode the attributes of an `<f t="dataTable" .../>` element into a [`DataTableFormula`].
fn parse_data_table_formula(e: &BytesStart<'_>) -> Result<DataTableFormula, XlsxError> {
    let mut range = None;
    let mut del1 = false;
    let mut del2 = false;
    let mut row_oriented = false;
    let mut r1_raw = None;
    let mut r2_raw = None;

    // Single pass over the attributes rather than re-scanning for each one.
    // xsd:boolean attributes accept "1" or "true" (Excel writes "1"; other writers may use "true").
    for attr in e.attributes() {
        let Attribute { key, value } = attr.map_err(XlsxError::XmlAttr)?;
        let Cow::Borrowed(value) = value else {
            continue;
        };
        match key.as_ref() {
            b"ref" => range = Some(get_dimension(value)?),
            b"del1" => del1 = matches!(value, b"1" | b"true"),
            b"del2" => del2 = matches!(value, b"1" | b"true"),
            b"dtr" => row_oriented = matches!(value, b"1" | b"true"),
            b"r1" => r1_raw = Some(value),
            b"r2" => r2_raw = Some(value),
            _ => {}
        }
    }

    let range = range.ok_or(XlsxError::Unexpected("dataTable <f> missing ref attribute"))?;

    let r1 = if del1 {
        None
    } else {
        r1_raw.map(get_row_column).transpose()?
    };
    let r2 = if del2 {
        None
    } else {
        r2_raw.map(get_row_column).transpose()?
    };

    // Two-variable iff r2 is present (not from dt2D — non-Excel writers may suppress it).
    let kind = if r2_raw.is_some() {
        DataTableKind::TwoVariable {
            row_input: r1,
            col_input: r2,
        }
    } else {
        DataTableKind::OneVariable {
            input: r1,
            orientation: if row_oriented {
                DataTableOrientation::Row
            } else {
                DataTableOrientation::Column
            },
        }
    };

    Ok(DataTableFormula { range, kind })
}

/// read the contents of a <v> cell
fn read_v<'s>(
    v: &mut String,
    strings: &'s [String],
    cell_format: Option<&CellFormat>,
    cell_type: Option<&[u8]>,
    is_1904: bool,
) -> Result<DataRef<'s>, XlsxError> {
    let cell_format = cell_format.or(Some(&CellFormat::Other));
    match cell_type {
        Some(b"s") => {
            // Cell value is an index into the shared string table.
            let idx = atoi_simd::parse::<usize>(v.as_bytes()).unwrap_or(0);
            match strings.get(idx) {
                Some(shared_string) => Ok(DataRef::SharedString(shared_string)),
                None => Err(XlsxError::Unexpected(
                    "Cell string index not found in shared strings table",
                )),
            }
        }
        Some(b"b") => {
            // boolean
            Ok(DataRef::Bool(v.as_str() != "0"))
        }
        Some(b"e") => {
            // error
            Ok(DataRef::Error(v.parse()?))
        }
        Some(b"d") => {
            // date
            Ok(DataRef::DateTimeIso(std::mem::take(v)))
        }
        Some(b"str") => {
            // string
            Ok(DataRef::String(std::mem::take(v)))
        }
        Some(b"n") => {
            // n - number
            if v.is_empty() {
                Ok(DataRef::Empty)
            } else {
                v.parse()
                    .map(|n| format_excel_f64_ref(n, cell_format, is_1904))
                    .map_err(XlsxError::ParseFloat)
            }
        }
        None => {
            // If type is not known, we try to parse as Float for utility, but fall back to
            // String if this fails.
            match v.parse() {
                Ok(n) => Ok(format_excel_f64_ref(n, cell_format, is_1904)),
                Err(_) => Ok(DataRef::String(std::mem::take(v))),
            }
        }
        Some(b"is") => {
            // this case should be handled in outer loop over cell elements, in which
            // case read_inline_str is called instead. Case included here for completeness.
            Err(XlsxError::Unexpected(
                "called read_value on a cell of type inlineStr",
            ))
        }
        Some(t) => {
            let t = std::str::from_utf8(t).unwrap_or("<utf8 error>").to_string();
            Err(XlsxError::CellTAttribute(t))
        }
    }
}

fn read_formula<RS>(
    xml: &mut XlReader<RS>,
    e: &BytesStart,
    event_buf: &mut Vec<u8>,
) -> Result<Option<String>, XlsxError>
where
    RS: Read + Seek,
{
    match e.local_name().as_ref() {
        b"is" | b"v" => {
            event_buf.clear();
            xml.read_to_end_into(e.name(), event_buf)?;
            Ok(None)
        }
        b"f" => {
            event_buf.clear();
            let mut f = String::new();
            loop {
                match xml.read_event_into(event_buf)? {
                    Event::Text(t) => f.push_str(&t.xml10_content()?),
                    Event::GeneralRef(e) => unescape_entity_to_buffer(&e, &mut f)?,
                    Event::End(end) if end.name() == e.name() => break,
                    Event::Eof => return Err(XlsxError::XmlEof("f")),
                    _ => (),
                }
                event_buf.clear();
            }
            Ok(Some(f))
        }
        _ => Err(XlsxError::UnexpectedNode("v, f, or is")),
    }
}
