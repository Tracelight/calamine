// SPDX-License-Identifier: MIT
//
// Copyright 2016-2025, Johann Tuffe.

use log::warn;
use quick_xml::{
    events::{BytesStart, Event},
    name::QName,
    Decoder,
};
use std::io::{Read, Seek};

use super::{
    get_attribute, get_dimension, get_row, get_row_column, parse_color_from_attrs, ColorPalette,
    XlReader, MAX_COLUMNS, MAX_ROWS,
};
use crate::{
    conditional_format::{
        ConditionalFormatOperator, ConditionalFormatRule, ConditionalFormatRuleType,
        ConditionalFormatTimePeriod, ConditionalFormatValue, ConditionalFormatValueKind,
        ConditionalFormatting,
    },
    utils::{decode_attr_value, unescape_entity_to_buffer},
    Dimensions, XlsxError,
};

pub(super) fn read_conditional_formatting<RS>(
    xml: &mut XlReader<'_, RS>,
    event_buf: &mut Vec<u8>,
    start: &BytesStart<'_>,
    palette: ColorPalette<'_>,
) -> Result<ConditionalFormatting, XlsxError>
where
    RS: Read + Seek,
{
    let ranges = match get_attribute(start.attributes(), QName(b"sqref"))? {
        Some(sqref) => parse_sqref(sqref),
        None => Vec::new(),
    };
    let pivot = get_attribute(start.attributes(), QName(b"pivot"))?
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(parse_xml_bool)
        .unwrap_or(false);
    let mut rules = Vec::new();
    let mut rule_buf = Vec::with_capacity(256);
    let mut child_buf = Vec::with_capacity(512);

    loop {
        event_buf.clear();
        match xml.read_event_into(event_buf) {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"cfRule" => {
                    rules.push(read_cf_rule(
                        xml,
                        &mut rule_buf,
                        &mut child_buf,
                        &e,
                        palette,
                    )?);
                }
                b"extLst" => {
                    child_buf.clear();
                    xml.read_to_end_into(e.name(), &mut child_buf)?;
                }
                _ => (),
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"conditionalFormatting" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("conditionalFormatting")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
    }

    Ok(ConditionalFormatting {
        pivot,
        ranges,
        rules,
    })
}

fn read_cf_rule<RS>(
    xml: &mut XlReader<'_, RS>,
    event_buf: &mut Vec<u8>,
    child_buf: &mut Vec<u8>,
    start: &BytesStart<'_>,
    palette: ColorPalette<'_>,
) -> Result<ConditionalFormatRule, XlsxError>
where
    RS: Read + Seek,
{
    let mut rule = read_cf_rule_attributes(xml.decoder(), start)?;

    // `XlReader` expands `<cfRule/>` into start/end events, so the end arm
    // handles childless rules.
    loop {
        event_buf.clear();
        match xml.read_event_into(event_buf) {
            Ok(Event::Start(e)) => match e.local_name().as_ref() {
                b"cfvo" => rule.values.push(read_cfvo(xml.decoder(), &e)?),
                b"color" => {
                    rule.colors
                        .push(parse_color_from_attrs(&e.attributes(), palette));
                }
                b"iconSet" | b"dataBar" => {
                    read_scale_attributes(xml.decoder(), &e, &mut rule)?;
                }
                b"formula" => {
                    rule.formulas
                        .push(read_element_text(xml, child_buf, &e, "formula")?);
                }
                b"extLst" => {
                    child_buf.clear();
                    xml.read_to_end_into(e.name(), child_buf)?;
                }
                _ => (),
            },
            Ok(Event::End(e)) if e.local_name().as_ref() == b"cfRule" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("cfRule")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => (),
        }
    }

    Ok(rule)
}

fn read_element_text<RS>(
    xml: &mut XlReader<'_, RS>,
    event_buf: &mut Vec<u8>,
    start: &BytesStart<'_>,
    eof_context: &'static str,
) -> Result<String, XlsxError>
where
    RS: Read + Seek,
{
    event_buf.clear();
    let mut text = String::new();
    loop {
        match xml.read_event_into(event_buf)? {
            Event::Text(value) => text.push_str(&value.xml10_content()?),
            Event::GeneralRef(entity) => unescape_entity_to_buffer(&entity, &mut text)?,
            Event::End(end) if end.name() == start.name() => break,
            Event::Eof => return Err(XlsxError::XmlEof(eof_context)),
            _ => (),
        }
        event_buf.clear();
    }
    Ok(text)
}

fn read_cf_rule_attributes(
    decoder: Decoder,
    start: &BytesStart<'_>,
) -> Result<ConditionalFormatRule, XlsxError> {
    let mut rule = ConditionalFormatRule::new(ConditionalFormatRuleType::Missing);
    for attr in start.attributes() {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        let value = decode_attr_value(&attr, decoder)?;
        match attr.key.local_name().as_ref() {
            b"type" => rule.rule_type = ConditionalFormatRuleType::from_attribute(&value),
            b"priority" => rule.priority = value.parse().ok(),
            b"stopIfTrue" => {
                rule.stop_if_true = parse_xml_bool(&value).unwrap_or(rule.stop_if_true);
            }
            b"dxfId" => rule.dxf_id = value.parse().ok(),
            b"operator" => {
                rule.operator = Some(ConditionalFormatOperator::from_attribute(&value));
            }
            b"text" => rule.text = Some(value.into_owned()),
            b"timePeriod" => {
                rule.time_period = Some(ConditionalFormatTimePeriod::from_attribute(&value));
            }
            b"rank" => rule.rank = value.parse().ok(),
            b"percent" => {
                rule.rank_percent = parse_xml_bool(&value).unwrap_or(rule.rank_percent);
            }
            b"bottom" => rule.bottom = parse_xml_bool(&value).unwrap_or(rule.bottom),
            b"aboveAverage" => {
                rule.above_average = parse_xml_bool(&value).unwrap_or(rule.above_average);
            }
            b"equalAverage" => {
                rule.equal_average = parse_xml_bool(&value).unwrap_or(rule.equal_average);
            }
            b"stdDev" => rule.std_dev = value.parse().ok(),
            _ => (),
        }
    }
    Ok(rule)
}

fn read_cfvo(
    decoder: Decoder,
    start: &BytesStart<'_>,
) -> Result<ConditionalFormatValue, XlsxError> {
    let mut cfvo = ConditionalFormatValue {
        kind: ConditionalFormatValueKind::Missing,
        value: None,
        inclusive: true,
    };
    for attr in start.attributes() {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        let value = decode_attr_value(&attr, decoder)?;
        match attr.key.local_name().as_ref() {
            b"type" => cfvo.kind = ConditionalFormatValueKind::from_attribute(&value),
            b"val" => cfvo.value = Some(value.into_owned()),
            b"gte" => cfvo.inclusive = parse_xml_bool(&value).unwrap_or(cfvo.inclusive),
            _ => (),
        }
    }
    Ok(cfvo)
}

fn read_scale_attributes(
    decoder: Decoder,
    start: &BytesStart<'_>,
    rule: &mut ConditionalFormatRule,
) -> Result<(), XlsxError> {
    let is_icon_set = start.local_name().as_ref() == b"iconSet";
    for attr in start.attributes() {
        let attr = attr.map_err(XlsxError::XmlAttr)?;
        let value = decode_attr_value(&attr, decoder)?;
        match attr.key.local_name().as_ref() {
            b"iconSet" if is_icon_set => rule.icon_set = Some(value.into_owned()),
            b"percent" if is_icon_set => {
                rule.icon_percent = parse_xml_bool(&value).unwrap_or(rule.icon_percent);
            }
            b"reverse" if is_icon_set => {
                rule.reverse_icons = parse_xml_bool(&value).unwrap_or(rule.reverse_icons);
            }
            b"minLength" if !is_icon_set => {
                rule.min_length = value.parse().unwrap_or(rule.min_length)
            }
            b"maxLength" if !is_icon_set => {
                rule.max_length = value.parse().unwrap_or(rule.max_length)
            }
            b"showValue" => {
                rule.show_value = parse_xml_bool(&value).unwrap_or(rule.show_value);
            }
            _ => (),
        }
    }
    Ok(())
}

fn parse_xml_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

fn parse_sqref(sqref: &[u8]) -> Vec<Dimensions> {
    sqref
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|area| !area.is_empty())
        .filter_map(|area| match parse_sqref_area(area) {
            Ok(dimension) => Some(dimension),
            Err(error) => {
                warn!(
                    "ignoring unparsable conditional formatting sqref area '{}': {error}",
                    String::from_utf8_lossy(area)
                );
                None
            }
        })
        .collect()
}

fn parse_sqref_area(area: &[u8]) -> Result<Dimensions, XlsxError> {
    let mut parts = area.split(|byte| *byte == b':');
    let start = parts.next().unwrap_or_default();
    let end = parts.next();
    if parts.next().is_some() {
        return get_dimension(area);
    }

    let Some(end) = end else {
        return get_dimension(area);
    };

    if start.iter().all(u8::is_ascii_alphabetic) && end.iter().all(u8::is_ascii_alphabetic) {
        return Ok(Dimensions {
            start: (0, column_index(start)?),
            end: (MAX_ROWS - 1, column_index(end)?),
        });
    }

    if start.iter().all(u8::is_ascii_digit) && end.iter().all(u8::is_ascii_digit) {
        return Ok(Dimensions {
            start: (get_row(start)?, 0),
            end: (get_row(end)?, MAX_COLUMNS - 1),
        });
    }

    get_dimension(area)
}

fn column_index(reference: &[u8]) -> Result<u32, XlsxError> {
    let mut cell_reference = Vec::with_capacity(reference.len() + 1);
    cell_reference.extend_from_slice(reference);
    cell_reference.push(b'1');
    get_row_column(&cell_reference).map(|(_, column)| column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::Reader;

    #[test]
    fn sqref_supports_cell_whole_column_and_whole_row_ranges() {
        assert_eq!(
            parse_sqref(b"A1:C3 B:D 2:4"),
            vec![
                Dimensions::new((0, 0), (2, 2)),
                Dimensions::new((0, 1), (MAX_ROWS - 1, 3)),
                Dimensions::new((1, 0), (3, MAX_COLUMNS - 1)),
            ]
        );
    }

    #[test]
    fn invalid_boolean_keeps_the_schema_default() {
        assert_eq!(parse_xml_bool("true"), Some(true));
        assert_eq!(parse_xml_bool("0"), Some(false));
        assert_eq!(parse_xml_bool("invalid"), None);
    }

    #[test]
    fn rule_and_scale_attributes_keep_types_and_defaults() {
        let decoder = Reader::from_str("").decoder();
        let rule_start = BytesStart::from_content(
            r#"cfRule type="aboveAverage" aboveAverage="invalid" stdDev="-2""#,
            6,
        );
        let rule = read_cf_rule_attributes(decoder, &rule_start).unwrap();
        assert_eq!(rule.rule_type, ConditionalFormatRuleType::AboveAverage);
        assert!(rule.above_average);
        assert_eq!(rule.std_dev, Some(-2));

        let mut data_bar = ConditionalFormatRule::new(ConditionalFormatRuleType::DataBar);
        let data_bar_start = BytesStart::from_content(
            r#"dataBar minLength="5" maxLength="95" showValue="invalid""#,
            7,
        );
        read_scale_attributes(decoder, &data_bar_start, &mut data_bar).unwrap();
        assert_eq!(data_bar.min_length, 5);
        assert_eq!(data_bar.max_length, 95);
        assert!(data_bar.show_value);

        let mut icon_set = ConditionalFormatRule::new(ConditionalFormatRuleType::IconSet);
        let icon_set_start = BytesStart::from_content(r#"iconSet percent="0" reverse="1""#, 7);
        read_scale_attributes(decoder, &icon_set_start, &mut icon_set).unwrap();
        assert!(!icon_set.icon_percent);
        assert!(icon_set.reverse_icons);
    }
}
