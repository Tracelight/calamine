// SPDX-License-Identifier: MIT
//
// Copyright 2016-2025, Johann Tuffe.

use quick_xml::{
    events::{attributes::Attribute, BytesStart, Event},
    name::QName,
    Reader,
};
use std::io::BufRead;

use crate::style::*;
use crate::utils::unescape_entity_to_buffer;
use crate::XlsxError;

use super::Theme;

#[inline]
fn parse_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

#[inline]
fn parse_hex_byte(bytes: &[u8]) -> Option<u8> {
    if bytes.len() != 2 { return None; }
    let hi = parse_hex_digit(bytes[0])?;
    let lo = parse_hex_digit(bytes[1])?;
    Some(hi * 16 + lo)
}

#[inline]
fn parse_u8_bytes(bytes: &[u8]) -> Option<u8> {
    if bytes.is_empty() { return None; }
    let mut result: u8 = 0;
    for &b in bytes {
        if b < b'0' || b > b'9' { return None; }
        result = result.checked_mul(10)?.checked_add(b - b'0')?;
    }
    Some(result)
}

#[inline]
fn parse_f64_bytes(bytes: &[u8]) -> Option<f64> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

/// Get theme color from Excel's theme color palette
/// Based on Office Open XML standard theme colors
pub(crate) fn get_theme_color(theme: u8) -> Color {
    match theme {
        0 => Color::rgb(255, 255, 255), // Light 1 (White)
        1 => Color::rgb(0, 0, 0),       // Dark 1 (Black)
        2 => Color::rgb(68, 84, 106),   // Light 2 (Light Gray)
        3 => Color::rgb(31, 73, 125),   // Dark 2 (Dark Blue)
        4 => Color::rgb(79, 129, 189),  // Accent 1 (Blue)
        5 => Color::rgb(192, 80, 77),   // Accent 2 (Red)
        6 => Color::rgb(155, 187, 89),  // Accent 3 (Green)
        7 => Color::rgb(128, 100, 162), // Accent 4 (Purple)
        8 => Color::rgb(75, 172, 198),  // Accent 5 (Cyan)
        9 => Color::rgb(247, 150, 70),  // Accent 6 (Orange)
        10 => Color::rgb(99, 99, 99),   // Hyperlink (Blue)
        11 => Color::rgb(128, 0, 128),  // Followed Hyperlink (Purple)
        _ => Color::rgb(0, 0, 0),       // Default to black for unknown theme colors
    }
}

const INDEXED_COLORS: [Color; 66] = [
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0x00 }, // 0: Black
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0xFF }, // 1: White
    Color { alpha: 255, red: 0xFF, green: 0x00, blue: 0x00 }, // 2: Red
    Color { alpha: 255, red: 0x00, green: 0xFF, blue: 0x00 }, // 3: Green
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0xFF }, // 4: Blue
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0x00 }, // 5: Yellow
    Color { alpha: 255, red: 0xFF, green: 0x00, blue: 0xFF }, // 6: Magenta
    Color { alpha: 255, red: 0x00, green: 0xFF, blue: 0xFF }, // 7: Cyan
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0x00 }, // 8: Black
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0xFF }, // 9: White
    Color { alpha: 255, red: 0xFF, green: 0x00, blue: 0x00 }, // 10: Red
    Color { alpha: 255, red: 0x00, green: 0xFF, blue: 0x00 }, // 11: Green
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0xFF }, // 12: Blue
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0x00 }, // 13: Yellow
    Color { alpha: 255, red: 0xFF, green: 0x00, blue: 0xFF }, // 14: Magenta
    Color { alpha: 255, red: 0x00, green: 0xFF, blue: 0xFF }, // 15: Cyan
    Color { alpha: 255, red: 0x80, green: 0x00, blue: 0x00 }, // 16: Maroon
    Color { alpha: 255, red: 0x00, green: 0x80, blue: 0x00 }, // 17: Dark Green
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0x80 }, // 18: Dark Blue
    Color { alpha: 255, red: 0x80, green: 0x80, blue: 0x00 }, // 19: Olive
    Color { alpha: 255, red: 0x80, green: 0x00, blue: 0x80 }, // 20: Purple
    Color { alpha: 255, red: 0x00, green: 0x80, blue: 0x80 }, // 21: Teal
    Color { alpha: 255, red: 0xC0, green: 0xC0, blue: 0xC0 }, // 22: Silver
    Color { alpha: 255, red: 0x80, green: 0x80, blue: 0x80 }, // 23: Gray
    Color { alpha: 255, red: 0x99, green: 0x99, blue: 0xFF }, // 24
    Color { alpha: 255, red: 0x99, green: 0x33, blue: 0x66 }, // 25
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0xCC }, // 26
    Color { alpha: 255, red: 0xCC, green: 0xFF, blue: 0xFF }, // 27
    Color { alpha: 255, red: 0x66, green: 0x00, blue: 0x66 }, // 28
    Color { alpha: 255, red: 0xFF, green: 0x80, blue: 0x80 }, // 29
    Color { alpha: 255, red: 0x00, green: 0x66, blue: 0xCC }, // 30
    Color { alpha: 255, red: 0xCC, green: 0xCC, blue: 0xFF }, // 31
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0x80 }, // 32
    Color { alpha: 255, red: 0xFF, green: 0x00, blue: 0xFF }, // 33
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0x00 }, // 34
    Color { alpha: 255, red: 0x00, green: 0xFF, blue: 0xFF }, // 35
    Color { alpha: 255, red: 0x80, green: 0x00, blue: 0x80 }, // 36
    Color { alpha: 255, red: 0x80, green: 0x00, blue: 0x00 }, // 37
    Color { alpha: 255, red: 0x00, green: 0x80, blue: 0x80 }, // 38
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0xFF }, // 39
    Color { alpha: 255, red: 0x00, green: 0xCC, blue: 0xFF }, // 40
    Color { alpha: 255, red: 0xCC, green: 0xFF, blue: 0xFF }, // 41
    Color { alpha: 255, red: 0xCC, green: 0xFF, blue: 0xCC }, // 42
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0x99 }, // 43
    Color { alpha: 255, red: 0x99, green: 0xCC, blue: 0xFF }, // 44
    Color { alpha: 255, red: 0xFF, green: 0x99, blue: 0xCC }, // 45
    Color { alpha: 255, red: 0xCC, green: 0x99, blue: 0xFF }, // 46
    Color { alpha: 255, red: 0xFF, green: 0xCC, blue: 0x99 }, // 47
    Color { alpha: 255, red: 0x33, green: 0x66, blue: 0xFF }, // 48
    Color { alpha: 255, red: 0x33, green: 0xCC, blue: 0xCC }, // 49
    Color { alpha: 255, red: 0x99, green: 0xCC, blue: 0x00 }, // 50
    Color { alpha: 255, red: 0xFF, green: 0xCC, blue: 0x00 }, // 51
    Color { alpha: 255, red: 0xFF, green: 0x99, blue: 0x00 }, // 52
    Color { alpha: 255, red: 0xFF, green: 0x66, blue: 0x00 }, // 53
    Color { alpha: 255, red: 0x66, green: 0x66, blue: 0x99 }, // 54
    Color { alpha: 255, red: 0x96, green: 0x96, blue: 0x96 }, // 55
    Color { alpha: 255, red: 0x00, green: 0x33, blue: 0x66 }, // 56
    Color { alpha: 255, red: 0x33, green: 0x99, blue: 0x66 }, // 57
    Color { alpha: 255, red: 0x00, green: 0x33, blue: 0x00 }, // 58
    Color { alpha: 255, red: 0x33, green: 0x33, blue: 0x00 }, // 59
    Color { alpha: 255, red: 0x99, green: 0x33, blue: 0x00 }, // 60
    Color { alpha: 255, red: 0x99, green: 0x33, blue: 0x66 }, // 61
    Color { alpha: 255, red: 0x33, green: 0x33, blue: 0x99 }, // 62
    Color { alpha: 255, red: 0x33, green: 0x33, blue: 0x33 }, // 63
    Color { alpha: 255, red: 0x00, green: 0x00, blue: 0x00 }, // 64: System Foreground
    Color { alpha: 255, red: 0xFF, green: 0xFF, blue: 0xFF }, // 65: System Background
];

fn get_indexed_color(index: u8, custom: Option<&[Color]>) -> Color {
    if let Some(colors) = custom {
        if let Some(&color) = colors.get(index as usize) {
            return color;
        }
    }
    INDEXED_COLORS.get(index as usize).copied().unwrap_or(Color::rgb(0, 0, 0))
}

/// Result of parsing a color: (color, from_theme)
/// from_theme is true if color came from theme attribute, false if from rgb/indexed
fn parse_color_with_source(
    attributes: &[Attribute],
    theme_data: Option<&Theme>,
    indexed_colors: Option<&[Color]>,
) -> Result<Option<(Color, bool)>, XlsxError> {
    let mut rgb_bytes: Option<&[u8]> = None;
    let mut theme_idx: Option<u8> = None;
    let mut indexed: Option<u8> = None;
    let mut tint: f64 = 0.0;

    for attr in attributes {
        match attr.key.as_ref() {
            b"rgb" => rgb_bytes = Some(&attr.value),
            b"theme" => theme_idx = parse_u8_bytes(&attr.value),
            b"indexed" => indexed = parse_u8_bytes(&attr.value),
            b"tint" => tint = parse_f64_bytes(&attr.value).unwrap_or(0.0),
            _ => {}
        }
    }

    if let Some(bytes) = rgb_bytes {
        let bytes = if bytes.first() == Some(&b'#') { &bytes[1..] } else { bytes };
        if bytes.len() == 8 {
            if let (Some(a), Some(r), Some(g), Some(b)) = (
                parse_hex_byte(&bytes[0..2]),
                parse_hex_byte(&bytes[2..4]),
                parse_hex_byte(&bytes[4..6]),
                parse_hex_byte(&bytes[6..8]),
            ) {
                let color = Color::new(a, r, g, b);
                return Ok(Some((if tint != 0.0 { color.with_tint(tint) } else { color }, false)));
            }
        } else if bytes.len() == 6 {
            if let (Some(r), Some(g), Some(b)) = (
                parse_hex_byte(&bytes[0..2]),
                parse_hex_byte(&bytes[2..4]),
                parse_hex_byte(&bytes[4..6]),
            ) {
                let color = Color::rgb(r, g, b);
                return Ok(Some((if tint != 0.0 { color.with_tint(tint) } else { color }, false)));
            }
        }
    }

    if let Some(idx) = theme_idx {
        let color = if let Some(theme) = theme_data {
            theme.color(idx as usize).unwrap_or_else(|| get_theme_color(idx))
        } else {
            get_theme_color(idx)
        };
        return Ok(Some((if tint != 0.0 { color.with_tint(tint) } else { color }, true)));
    }

    if let Some(idx) = indexed {
        let color = get_indexed_color(idx, indexed_colors);
        return Ok(Some((if tint != 0.0 { color.with_tint(tint) } else { color }, false)));
    }

    Ok(None)
}

fn parse_color_with_theme(
    attributes: &[Attribute],
    theme_data: Option<&Theme>,
    indexed_colors: Option<&[Color]>,
) -> Result<Option<Color>, XlsxError> {
    Ok(parse_color_with_source(attributes, theme_data, indexed_colors)?.map(|(c, _)| c))
}

/// Parse font weight from string
fn parse_font_weight(s: &str) -> FontWeight {
    match s {
        "bold" | "700" => FontWeight::Bold,
        "normal" | "400" => FontWeight::Normal,
        _ => {
            // Try to parse as numeric weight
            if let Ok(weight) = s.parse::<u16>() {
                if weight >= 600 {
                    FontWeight::Bold
                } else {
                    FontWeight::Normal
                }
            } else {
                FontWeight::Normal
            }
        }
    }
}

/// Parse font style from string
fn parse_font_style(s: &str) -> FontStyle {
    match s {
        "italic" | "oblique" => FontStyle::Italic,
        "normal" => FontStyle::Normal,
        _ => FontStyle::Normal,
    }
}

/// Parse underline style from string
fn parse_underline_style(s: &str) -> UnderlineStyle {
    match s {
        "single" => UnderlineStyle::Single,
        "double" => UnderlineStyle::Double,
        "singleAccounting" => UnderlineStyle::SingleAccounting,
        "doubleAccounting" => UnderlineStyle::DoubleAccounting,
        _ => UnderlineStyle::None,
    }
}

/// Parse horizontal alignment from string
fn parse_horizontal_alignment(s: &str) -> HorizontalAlignment {
    match s {
        "left" => HorizontalAlignment::Left,
        "center" => HorizontalAlignment::Center,
        "right" => HorizontalAlignment::Right,
        "justify" => HorizontalAlignment::Justify,
        "distributed" => HorizontalAlignment::Distributed,
        "fill" => HorizontalAlignment::Fill,
        "centerContinuous" => HorizontalAlignment::CenterContinuous,
        _ => HorizontalAlignment::General,
    }
}

/// Parse vertical alignment from string
fn parse_vertical_alignment(s: &str) -> VerticalAlignment {
    match s {
        "top" => VerticalAlignment::Top,
        "center" => VerticalAlignment::Center,
        "bottom" => VerticalAlignment::Bottom,
        "justify" => VerticalAlignment::Justify,
        "distributed" => VerticalAlignment::Distributed,
        _ => VerticalAlignment::Bottom,
    }
}

/// Parse fill pattern from string
fn parse_fill_pattern(s: &str) -> FillPattern {
    match s {
        "solid" => FillPattern::Solid,
        "darkGray" => FillPattern::DarkGray,
        "mediumGray" => FillPattern::MediumGray,
        "lightGray" => FillPattern::LightGray,
        "gray125" => FillPattern::Gray125,
        "gray0625" => FillPattern::Gray0625,
        "darkHorizontal" => FillPattern::DarkHorizontal,
        "darkVertical" => FillPattern::DarkVertical,
        "darkDown" => FillPattern::DarkDown,
        "darkUp" => FillPattern::DarkUp,
        "darkGrid" => FillPattern::DarkGrid,
        "darkTrellis" => FillPattern::DarkTrellis,
        "lightHorizontal" => FillPattern::LightHorizontal,
        "lightVertical" => FillPattern::LightVertical,
        "lightDown" => FillPattern::LightDown,
        "lightUp" => FillPattern::LightUp,
        "lightGrid" => FillPattern::LightGrid,
        "lightTrellis" => FillPattern::LightTrellis,
        _ => FillPattern::None,
    }
}

/// Parse border style from string
fn parse_border_style(s: &str) -> BorderStyle {
    match s {
        "thin" => BorderStyle::Thin,
        "medium" => BorderStyle::Medium,
        "thick" => BorderStyle::Thick,
        "double" => BorderStyle::Double,
        "hair" => BorderStyle::Hair,
        "dashed" => BorderStyle::Dashed,
        "dotted" => BorderStyle::Dotted,
        "mediumDashed" => BorderStyle::MediumDashed,
        "dashDot" => BorderStyle::DashDot,
        "mediumDashDot" => BorderStyle::MediumDashDot,
        "dashDotDot" => BorderStyle::DashDotDot,
        "mediumDashDotDot" => BorderStyle::MediumDashDotDot,
        "slantDashDot" => BorderStyle::SlantDashDot,
        _ => BorderStyle::None,
    }
}

pub fn parse_font_with_theme<RS: BufRead>(
    xml: &mut Reader<RS>,
    _start_elem: &BytesStart,
    theme: Option<&Theme>,
    indexed_colors: Option<&[Color]>,
) -> Result<Font, XlsxError> {
    let mut font = Font::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                b"name" => {
                    let mut name = None;
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"val" {
                            name = Some(String::from_utf8_lossy(&attr.value).to_string());
                            break;
                        }
                    }
                    if name.is_none() {
                        name = read_string(xml, QName(b"name"))?;
                    } else {
                        xml.read_to_end_into(e.name(), &mut Vec::new())?;
                    }
                    if let Some(n) = name {
                        font = font.with_name(n);
                    }
                }
                b"sz" => {
                    let mut size_str = None;
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"val" {
                            size_str = Some(String::from_utf8_lossy(&attr.value).to_string());
                            break;
                        }
                    }
                    if size_str.is_none() {
                        size_str = read_string(xml, QName(b"sz"))?;
                    } else {
                        xml.read_to_end_into(e.name(), &mut Vec::new())?;
                    }
                    if let Some(s) = size_str {
                        if let Ok(size) = s.parse::<f64>() {
                            font = font.with_size(size);
                        }
                    }
                }
                b"b" => {
                    let mut weight = FontWeight::Bold;
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"val" {
                            let val_str = String::from_utf8_lossy(&attr.value);
                            weight = parse_font_weight(&val_str);
                            break;
                        }
                    }
                    font = font.with_weight(weight);
                }
                b"i" => {
                    let mut style = FontStyle::Italic;
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"val" {
                            let val_str = String::from_utf8_lossy(&attr.value);
                            style = parse_font_style(&val_str);
                            break;
                        }
                    }
                    font = font.with_style(style);
                }
                b"u" => {
                    let mut underline_style = UnderlineStyle::Single;
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"val" {
                            let val_str = String::from_utf8_lossy(&attr.value);
                            underline_style = parse_underline_style(&val_str);
                            break;
                        }
                    }
                    font = font.with_underline(underline_style);
                }
                b"strike" => {
                    font = font.with_strikethrough(true);
                }
                b"color" => {
                    if let Some((color, from_theme)) =
                        parse_color_with_source(&e.attributes().collect::<Result<Vec<_>, _>>()?, theme, indexed_colors)?
                    {
                        font = font.with_color_and_source(color, from_theme);
                    }
                }
                b"family" => {
                    if let Some(family) = read_string(xml, QName(b"family"))? {
                        font = font.with_family(family);
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.local_name().as_ref() == b"font" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("font")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => {}
        }
    }

    Ok(font)
}

pub fn parse_fill_with_theme<RS: BufRead>(
    xml: &mut Reader<RS>,
    _start_elem: &BytesStart,
    theme: Option<&Theme>,
    indexed_colors: Option<&[Color]>,
) -> Result<Fill, XlsxError> {
    let mut fill = Fill::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => match e.local_name().as_ref() {
                b"patternFill" => {
                    for attr in e.attributes() {
                        let attr = attr?;
                        if attr.key.as_ref() == b"patternType" {
                            let pattern_str = String::from_utf8_lossy(&attr.value);
                            fill = fill.with_pattern(parse_fill_pattern(&pattern_str));
                        }
                    }
                }
                b"fgColor" => {
                    if let Some(color) =
                        parse_color_with_theme(&e.attributes().collect::<Result<Vec<_>, _>>()?, theme, indexed_colors)?
                    {
                        fill = fill.with_foreground_color(color);
                    }
                }
                b"bgColor" => {
                    if let Some(color) =
                        parse_color_with_theme(&e.attributes().collect::<Result<Vec<_>, _>>()?, theme, indexed_colors)?
                    {
                        fill = fill.with_background_color(color);
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.local_name().as_ref() == b"fill" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("fill")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => {}
        }
    }

    Ok(fill)
}

pub fn parse_border_with_theme<RS: BufRead>(
    xml: &mut Reader<RS>,
    _start_elem: &BytesStart,
    theme: Option<&Theme>,
    indexed_colors: Option<&[Color]>,
) -> Result<Borders, XlsxError> {
    let mut borders = Borders::new();
    let mut buf = Vec::new();

    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                match e.local_name().as_ref() {
                    b"left" | b"right" | b"top" | b"bottom" | b"diagonal" => {
                        let mut style = BorderStyle::None;
                        let mut color = None;

                        for attr in e.attributes() {
                            let attr = attr?;
                            if attr.key.as_ref() == b"style" {
                                let style_str = String::from_utf8_lossy(&attr.value);
                                style = parse_border_style(&style_str);
                            }
                        }

                        if let Some(border_color) =
                            parse_color_with_theme(&e.attributes().collect::<Result<Vec<_>, _>>()?, theme, indexed_colors)?
                        {
                            color = Some(border_color);
                        }

                        let mut inner_buf = Vec::new();
                        loop {
                            inner_buf.clear();
                            match xml.read_event_into(&mut inner_buf) {
                                Ok(Event::Start(ref inner_e) | Event::Empty(ref inner_e)) => {
                                    if inner_e.local_name().as_ref() == b"color" {
                                        if let Some(border_color) = parse_color_with_theme(
                                            &inner_e.attributes().collect::<Result<Vec<_>, _>>()?,
                                            theme,
                                            indexed_colors,
                                        )? {
                                            color = Some(border_color);
                                        }
                                    }
                                }
                                Ok(Event::End(ref inner_e))
                                    if inner_e.local_name().as_ref() == e.local_name().as_ref() =>
                                {
                                    break
                                }
                                Ok(Event::Eof) => return Err(XlsxError::XmlEof("border side")),
                                Err(e) => return Err(XlsxError::Xml(e)),
                                _ => {}
                            }
                        }

                        let border = if let Some(c) = color {
                            Border::with_color(style, c)
                        } else {
                            Border::new(style)
                        };

                        match e.local_name().as_ref() {
                            b"left" => borders.left = border,
                            b"right" => borders.right = border,
                            b"top" => borders.top = border,
                            b"bottom" => borders.bottom = border,
                            b"diagonal" => {
                                for attr in e.attributes() {
                                    let attr = attr?;
                                    if attr.key.as_ref() == b"diagonalDown" {
                                        borders.diagonal_down = border.clone();
                                    } else if attr.key.as_ref() == b"diagonalUp" {
                                        borders.diagonal_up = border.clone();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(ref e)) => {
                match e.local_name().as_ref() {
                    b"left" | b"right" | b"top" | b"bottom" | b"diagonal" => {
                        let mut style = BorderStyle::None;
                        let mut color = None;

                        for attr in e.attributes() {
                            let attr = attr?;
                            if attr.key.as_ref() == b"style" {
                                let style_str = String::from_utf8_lossy(&attr.value);
                                style = parse_border_style(&style_str);
                            }
                        }

                        if let Some(border_color) =
                            parse_color_with_theme(&e.attributes().collect::<Result<Vec<_>, _>>()?, theme, indexed_colors)?
                        {
                            color = Some(border_color);
                        }

                        let border = if let Some(c) = color {
                            Border::with_color(style, c)
                        } else {
                            Border::new(style)
                        };

                        match e.local_name().as_ref() {
                            b"left" => borders.left = border,
                            b"right" => borders.right = border,
                            b"top" => borders.top = border,
                            b"bottom" => borders.bottom = border,
                            b"diagonal" => {
                                for attr in e.attributes() {
                                    let attr = attr?;
                                    if attr.key.as_ref() == b"diagonalDown" {
                                        borders.diagonal_down = border.clone();
                                    } else if attr.key.as_ref() == b"diagonalUp" {
                                        borders.diagonal_up = border.clone();
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) if e.local_name().as_ref() == b"border" => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("border")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => {}
        }
    }

    Ok(borders)
}

/// Parse alignment element
pub fn parse_alignment<RS: BufRead>(
    _xml: &mut Reader<RS>,
    start_elem: &BytesStart,
) -> Result<Alignment, XlsxError> {
    let mut alignment = Alignment::new();

    for attr in start_elem.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            b"horizontal" => {
                let horizontal_str = String::from_utf8_lossy(&attr.value);
                alignment = alignment.with_horizontal(parse_horizontal_alignment(&horizontal_str));
            }
            b"vertical" => {
                let vertical_str = String::from_utf8_lossy(&attr.value);
                alignment = alignment.with_vertical(parse_vertical_alignment(&vertical_str));
            }
            b"wrapText" => {
                let wrap_str = String::from_utf8_lossy(&attr.value);
                if wrap_str == "1" || wrap_str == "true" {
                    alignment = alignment.with_wrap_text(true);
                }
            }
            b"textRotation" => {
                if let Ok(rotation) = String::from_utf8_lossy(&attr.value).parse::<u16>() {
                    alignment = alignment.with_text_rotation(TextRotation::Degrees(rotation));
                }
            }
            b"indent" => {
                if let Ok(indent) = String::from_utf8_lossy(&attr.value).parse::<u8>() {
                    alignment = alignment.with_indent(indent);
                }
            }
            b"shrinkToFit" => {
                let shrink_str = String::from_utf8_lossy(&attr.value);
                if shrink_str == "1" || shrink_str == "true" {
                    alignment = alignment.with_shrink_to_fit(true);
                }
            }
            _ => {}
        }
    }

    Ok(alignment)
}

/// Parse protection element
pub fn parse_protection<RS: BufRead>(
    _xml: &mut Reader<RS>,
    start_elem: &BytesStart,
) -> Result<Protection, XlsxError> {
    let mut protection = Protection::new();

    for attr in start_elem.attributes() {
        let attr = attr?;
        match attr.key.as_ref() {
            b"locked" => {
                let locked_str = String::from_utf8_lossy(&attr.value);
                if locked_str == "1" || locked_str == "true" {
                    protection = protection.with_locked(true);
                }
            }
            b"hidden" => {
                let hidden_str = String::from_utf8_lossy(&attr.value);
                if hidden_str == "1" || hidden_str == "true" {
                    protection = protection.with_hidden(true);
                }
            }
            _ => {}
        }
    }

    Ok(protection)
}
/// Read string content from XML element
fn read_string<RS: BufRead>(
    xml: &mut Reader<RS>,
    closing: QName,
) -> Result<Option<String>, XlsxError> {
    let mut buf = Vec::new();
    let mut content = String::new();

    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Text(e)) => {
                content.push_str(&e.xml10_content()?);
            }
            Ok(Event::GeneralRef(e)) => {
                unescape_entity_to_buffer(&e, &mut content)?;
            }
            Ok(Event::End(ref e)) if e.local_name() == closing.into() => break,
            Ok(Event::Eof) => return Err(XlsxError::XmlEof("string")),
            Err(e) => return Err(XlsxError::Xml(e)),
            _ => {}
        }
    }

    if content.is_empty() {
        Ok(None)
    } else {
        Ok(Some(content))
    }
}
