// SPDX-License-Identifier: MIT
//
// Copyright 2016-2025, Johann Tuffe.

//! Minimal chart extraction from an XLSX worksheet's drawing and chart parts.
//!
//! Returns the raw OOXML fields as strings (chart-type wrapper tag, series formula
//! references, anchor cells) without interpreting them. Consumers map these onto their
//! own chart-type names / coordinate types.

use std::collections::HashMap;
use std::io::{Read, Seek};

use log::warn;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use zip::read::ZipArchive;

use super::xml_reader;
use crate::utils::unescape_entity_to_buffer;

/// A data series within a chart. References point at the source data as raw A1 formulas
/// (e.g. `Sheet1!$A$2:$A$7`), exactly as stored in the chart XML.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChartSeries {
    /// Series name, if set via a literal `<c:tx><c:v>` value.
    pub name: Option<String>,
    /// Category axis reference (`<c:cat>`).
    pub categories_ref: Option<String>,
    /// Value axis reference (`<c:val>`).
    pub values_ref: Option<String>,
    /// X-value reference for scatter/bubble charts (`<c:xVal>`).
    pub x_values_ref: Option<String>,
    /// Y-value reference for scatter/bubble charts (`<c:yVal>`).
    pub y_values_ref: Option<String>,
}

/// How a chart is placed on a worksheet. DrawingML defines three anchor flavors and
/// the variant preserves which one the file uses, with the raw coordinates as stored.
///
/// EMU = English Metric Units (1 cm = 360_000 EMU, 1 in = 914_400 EMU); signed because
/// DrawingML permits negative offsets and positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartAnchor {
    /// `<xdr:twoCellAnchor>` — both corners pinned to cells. 0-indexed.
    TwoCell {
        /// 0-indexed column of the top-left anchor cell.
        from_col: u32,
        /// 0-indexed row of the top-left anchor cell.
        from_row: u32,
        /// 0-indexed column of the bottom-right anchor cell.
        to_col: u32,
        /// 0-indexed row of the bottom-right anchor cell.
        to_row: u32,
    },
    /// `<xdr:oneCellAnchor>` — top-left pinned to a cell (0-indexed); size given as
    /// `<xdr:ext cx cy>` in EMUs.
    OneCell {
        /// 0-indexed column of the top-left anchor cell.
        from_col: u32,
        /// 0-indexed row of the top-left anchor cell.
        from_row: u32,
        /// Width in EMUs (`<xdr:ext cx>`).
        ext_cx_emu: i64,
        /// Height in EMUs (`<xdr:ext cy>`).
        ext_cy_emu: i64,
    },
    /// `<xdr:absoluteAnchor>` — position and size both given in EMUs; no cell anchoring.
    Absolute {
        /// Horizontal position in EMUs (`<xdr:pos x>`).
        pos_x_emu: i64,
        /// Vertical position in EMUs (`<xdr:pos y>`).
        pos_y_emu: i64,
        /// Width in EMUs (`<xdr:ext cx>`).
        ext_cx_emu: i64,
        /// Height in EMUs (`<xdr:ext cy>`).
        ext_cy_emu: i64,
    },
}

/// A chart anchored on a worksheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chart {
    /// Stable identifier from `<a16:creationId>` (a GUID like `{...}`), when present.
    /// Office-authored files set this and it matches the id surfaced by the Office.js API;
    /// older or third-party files may omit it.
    pub id: Option<String>,
    /// Shape name from `<xdr:cNvPr name>`.
    pub name: Option<String>,
    /// Chart title text, if present.
    pub title: Option<String>,
    /// Raw chart-type wrapper tag, e.g. `"barChart"`, `"lineChart"`, `"scatterChart"`.
    pub chart_type: Option<String>,
    /// Placement on the worksheet, when parsable. Carries which DrawingML anchor flavor
    /// was used. `None` if the anchor is malformed or missing required coordinates.
    pub anchor: Option<ChartAnchor>,
    /// Data series.
    pub series: Vec<ChartSeries>,
}

/// Parse all charts anchored on the worksheet stored at `sheet_path` (e.g.
/// `xl/worksheets/sheet1.xml`). Best-effort: unreadable or malformed parts are skipped.
pub(super) fn read_charts_for_sheet<RS: Read + Seek>(
    zip: &mut ZipArchive<RS>,
    sheet_path: &str,
) -> Vec<Chart> {
    let Some(rels_path) = sheet_rels_path(sheet_path) else {
        return Vec::new();
    };
    let drawing_paths = read_drawing_paths(zip, &rels_path, sheet_path);
    let mut charts = Vec::new();
    for drawing_path in drawing_paths {
        let frames = read_drawing_frames(zip, &drawing_path);
        if frames.is_empty() {
            continue;
        }
        let chart_part_paths = read_drawing_chart_part_paths(zip, &drawing_path);
        for frame in frames {
            let chart_part_path = frame
                .chart_part_rid
                .as_deref()
                .and_then(|rid| chart_part_paths.get(rid).cloned());
            let info = chart_part_path
                .as_deref()
                .map(|path| parse_chart_part(zip, path))
                .unwrap_or_default();
            charts.push(Chart {
                id: frame.creation_id,
                name: frame.name,
                title: info.title,
                chart_type: info.chart_type,
                anchor: frame.anchor,
                series: info.series,
            });
        }
    }
    charts
}

fn sheet_rels_path(sheet_path: &str) -> Option<String> {
    let (dir, file) = sheet_path.rsplit_once('/')?;
    Some(format!("{dir}/_rels/{file}.rels"))
}

fn drawing_rels_path(drawing_path: &str) -> Option<String> {
    let (dir, file) = drawing_path.rsplit_once('/')?;
    Some(format!("{dir}/_rels/{file}.rels"))
}

fn resolve_relative_target(base_path: &str, target: &str) -> Option<String> {
    if let Some(rest) = target.strip_prefix("/xl/") {
        return Some(format!("xl/{rest}"));
    }
    if target.starts_with("xl/") {
        return Some(target.to_string());
    }
    let base_dir = base_path.rsplit_once('/')?.0;
    let mut parts: Vec<&str> = base_dir.split('/').collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn read_drawing_paths<RS: Read + Seek>(
    zip: &mut ZipArchive<RS>,
    rels_path: &str,
    base_sheet_path: &str,
) -> Vec<String> {
    let mut xml = match xml_reader(zip, rels_path) {
        Some(Ok(reader)) => reader,
        Some(Err(e)) => {
            warn!("Failed to open sheet rels {rels_path}: {e}");
            return Vec::new();
        }
        None => return Vec::new(),
    };
    let mut drawings = Vec::new();
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                let mut is_drawing = false;
                let mut target = String::new();
                for a in e.attributes().flatten() {
                    match a.key {
                        QName(b"Type") if a.value.ends_with(b"/drawing") => {
                            is_drawing = true;
                        }
                        QName(b"Target") => {
                            target = xml
                                .decoder()
                                .decode(&a.value)
                                .unwrap_or_default()
                                .into_owned();
                        }
                        _ => {}
                    }
                }
                if is_drawing && !target.is_empty() {
                    if let Some(resolved) = resolve_relative_target(base_sheet_path, &target) {
                        drawings.push(resolved);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    drawings
}

fn read_drawing_chart_part_paths<RS: Read + Seek>(
    zip: &mut ZipArchive<RS>,
    drawing_path: &str,
) -> HashMap<String, String> {
    let Some(rels_path) = drawing_rels_path(drawing_path) else {
        return HashMap::new();
    };
    let mut xml = match xml_reader(zip, &rels_path) {
        Some(Ok(reader)) => reader,
        Some(Err(e)) => {
            warn!("Failed to open drawing rels {rels_path}: {e}");
            return HashMap::new();
        }
        None => return HashMap::new(),
    };
    let mut map = HashMap::new();
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        match xml.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                let mut is_chart = false;
                let mut rid = String::new();
                let mut target = String::new();
                for a in e.attributes().flatten() {
                    match a.key {
                        QName(b"Type") if a.value.ends_with(b"/chart") => {
                            is_chart = true;
                        }
                        QName(b"Id") => {
                            rid = xml
                                .decoder()
                                .decode(&a.value)
                                .unwrap_or_default()
                                .into_owned();
                        }
                        QName(b"Target") => {
                            target = xml
                                .decoder()
                                .decode(&a.value)
                                .unwrap_or_default()
                                .into_owned();
                        }
                        _ => {}
                    }
                }
                if is_chart && !rid.is_empty() && !target.is_empty() {
                    if let Some(resolved) = resolve_relative_target(drawing_path, &target) {
                        map.insert(rid, resolved);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    map
}

#[derive(Debug, Default)]
struct DrawingFrame {
    creation_id: Option<String>,
    name: Option<String>,
    anchor: Option<ChartAnchor>,
    chart_part_rid: Option<String>,
}

#[derive(Debug, Copy, Clone)]
enum TextTarget {
    Col,
    Row,
}

#[derive(Debug, Copy, Clone)]
enum AnchorKind {
    TwoCell,
    OneCell,
    Absolute,
}

fn read_drawing_frames<RS: Read + Seek>(
    zip: &mut ZipArchive<RS>,
    drawing_path: &str,
) -> Vec<DrawingFrame> {
    let mut xml = match xml_reader(zip, drawing_path) {
        Some(Ok(reader)) => reader,
        Some(Err(e)) => {
            warn!("Failed to open drawing {drawing_path}: {e}");
            return Vec::new();
        }
        None => return Vec::new(),
    };
    let mut frames = Vec::new();
    let mut buf = Vec::with_capacity(1024);

    let mut current_anchor: Option<AnchorKind> = None;
    let mut anchor_from: Option<(u32, u32)> = None;
    let mut anchor_to: Option<(u32, u32)> = None;
    let mut anchor_pos_emu: Option<(i64, i64)> = None;
    let mut anchor_ext_emu: Option<(i64, i64)> = None;
    let mut in_from = false;
    let mut in_to = false;
    let mut pending_col: Option<u32> = None;
    let mut pending_row: Option<u32> = None;
    let mut text_target: Option<TextTarget> = None;
    let mut text_buf: String = String::new();

    let mut in_graphic_frame = false;
    let mut frame_name: Option<String> = None;
    let mut frame_has_chart = false;
    let mut frame_creation_id: Option<String> = None;
    let mut frame_chart_rid: Option<String> = None;

    loop {
        buf.clear();
        let event = xml.read_event_into(&mut buf);
        let is_start = matches!(&event, Ok(Event::Start(_)));
        let decoder = xml.decoder();
        match event {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let name = e.local_name();
                let n = name.as_ref();
                match n {
                    b"twoCellAnchor" if is_start => {
                        current_anchor = Some(AnchorKind::TwoCell);
                        anchor_from = None;
                        anchor_to = None;
                        anchor_pos_emu = None;
                        anchor_ext_emu = None;
                    }
                    b"oneCellAnchor" if is_start => {
                        current_anchor = Some(AnchorKind::OneCell);
                        anchor_from = None;
                        anchor_to = None;
                        anchor_pos_emu = None;
                        anchor_ext_emu = None;
                    }
                    b"absoluteAnchor" if is_start => {
                        current_anchor = Some(AnchorKind::Absolute);
                        anchor_from = None;
                        anchor_to = None;
                        anchor_pos_emu = None;
                        anchor_ext_emu = None;
                    }
                    b"from" if is_start && current_anchor.is_some() => {
                        in_from = true;
                        pending_col = None;
                        pending_row = None;
                    }
                    b"to" if is_start && current_anchor.is_some() => {
                        in_to = true;
                        pending_col = None;
                        pending_row = None;
                    }
                    b"col" if is_start && (in_from || in_to) => {
                        text_target = Some(TextTarget::Col);
                        text_buf.clear();
                    }
                    b"row" if is_start && (in_from || in_to) => {
                        text_target = Some(TextTarget::Row);
                        text_buf.clear();
                    }
                    b"pos" if current_anchor.is_some() && !in_graphic_frame => {
                        anchor_pos_emu = parse_xy_emu(&e, decoder, b"x", b"y");
                    }
                    b"ext" if current_anchor.is_some() && !in_graphic_frame => {
                        anchor_ext_emu = parse_xy_emu(&e, decoder, b"cx", b"cy");
                    }
                    b"graphicFrame" if is_start => {
                        in_graphic_frame = true;
                        frame_name = None;
                        frame_has_chart = false;
                        frame_creation_id = None;
                        frame_chart_rid = None;
                    }
                    b"cNvPr" if in_graphic_frame => {
                        for a in e.attributes().flatten() {
                            if a.key == QName(b"name") {
                                if let Ok(v) = a.decode_and_unescape_value(decoder) {
                                    frame_name = Some(v.into_owned());
                                }
                            }
                        }
                    }
                    b"creationId" if in_graphic_frame => {
                        for a in e.attributes().flatten() {
                            if a.key.local_name().as_ref() == b"id" {
                                if let Ok(v) = decoder.decode(&a.value) {
                                    frame_creation_id = Some(v.into_owned());
                                }
                            }
                        }
                    }
                    b"graphicData" if in_graphic_frame && graphic_data_is_chart(&e) => {
                        frame_has_chart = true;
                    }
                    b"chart" if in_graphic_frame && frame_has_chart => {
                        for a in e.attributes().flatten() {
                            if a.key.local_name().as_ref() == b"id" {
                                if let Ok(v) = decoder.decode(&a.value) {
                                    frame_chart_rid = Some(v.into_owned());
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) if text_target.is_some() => {
                if let Ok(s) = decoder.decode(&t) {
                    text_buf.push_str(&s);
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                let n = name.as_ref();
                if text_target.is_some() && (n == b"col" || n == b"row") {
                    let trimmed = text_buf.trim();
                    if !trimmed.is_empty() {
                        match trimmed.parse::<u32>() {
                            Ok(v) => match text_target {
                                Some(TextTarget::Col) => pending_col = Some(v),
                                Some(TextTarget::Row) => pending_row = Some(v),
                                None => {}
                            },
                            Err(_) => {
                                warn!(
                                    "Failed to parse anchor col/row in {drawing_path}: {trimmed}"
                                );
                            }
                        }
                    }
                    text_target = None;
                    text_buf.clear();
                } else if n == b"from" && in_from {
                    if let (Some(c), Some(r)) = (pending_col, pending_row) {
                        anchor_from = Some((c, r));
                    }
                    in_from = false;
                    pending_col = None;
                    pending_row = None;
                } else if n == b"to" && in_to {
                    if let (Some(c), Some(r)) = (pending_col, pending_row) {
                        anchor_to = Some((c, r));
                    }
                    in_to = false;
                    pending_col = None;
                    pending_row = None;
                } else if n == b"graphicFrame" && in_graphic_frame {
                    if frame_has_chart {
                        let anchor = build_anchor(
                            current_anchor,
                            anchor_from,
                            anchor_to,
                            anchor_pos_emu,
                            anchor_ext_emu,
                        );
                        frames.push(DrawingFrame {
                            creation_id: frame_creation_id.take(),
                            name: frame_name.take(),
                            anchor,
                            chart_part_rid: frame_chart_rid.take(),
                        });
                    }
                    in_graphic_frame = false;
                    frame_name = None;
                    frame_has_chart = false;
                    frame_creation_id = None;
                    frame_chart_rid = None;
                } else if (n == b"twoCellAnchor" || n == b"oneCellAnchor" || n == b"absoluteAnchor")
                    && current_anchor.is_some()
                {
                    current_anchor = None;
                    anchor_from = None;
                    anchor_to = None;
                    anchor_pos_emu = None;
                    anchor_ext_emu = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("XML parse error in drawing {drawing_path}: {e}");
                break;
            }
            _ => {}
        }
    }

    frames
}

fn build_anchor(
    kind: Option<AnchorKind>,
    from: Option<(u32, u32)>,
    to: Option<(u32, u32)>,
    pos: Option<(i64, i64)>,
    ext: Option<(i64, i64)>,
) -> Option<ChartAnchor> {
    match kind? {
        AnchorKind::TwoCell => {
            let (fc, fr) = from?;
            let (tc, tr) = to?;
            Some(ChartAnchor::TwoCell {
                from_col: fc,
                from_row: fr,
                to_col: tc,
                to_row: tr,
            })
        }
        AnchorKind::OneCell => {
            let (fc, fr) = from?;
            let (cx, cy) = ext?;
            Some(ChartAnchor::OneCell {
                from_col: fc,
                from_row: fr,
                ext_cx_emu: cx,
                ext_cy_emu: cy,
            })
        }
        AnchorKind::Absolute => {
            let (x, y) = pos?;
            let (cx, cy) = ext?;
            Some(ChartAnchor::Absolute {
                pos_x_emu: x,
                pos_y_emu: y,
                ext_cx_emu: cx,
                ext_cy_emu: cy,
            })
        }
    }
}

fn parse_xy_emu(
    e: &BytesStart,
    decoder: quick_xml::Decoder,
    x_key: &[u8],
    y_key: &[u8],
) -> Option<(i64, i64)> {
    let mut x: Option<i64> = None;
    let mut y: Option<i64> = None;
    for a in e.attributes().flatten() {
        let k = a.key.local_name();
        let kref = k.as_ref();
        if kref == x_key || kref == y_key {
            let Ok(s) = decoder.decode(&a.value) else {
                continue;
            };
            let Ok(v) = s.trim().parse::<i64>() else {
                continue;
            };
            if kref == x_key {
                x = Some(v);
            } else {
                y = Some(v);
            }
        }
    }
    Some((x?, y?))
}

/// The `a:graphicData/@uri` values that identify an embedded chart. Per ISO/IEC
/// 29500 these are the two equivalent DrawingML chart namespaces: the ECMA-376
/// transitional one and the strict (purl.oclc.org) one.
const CHART_GRAPHIC_DATA_URIS: [&[u8]; 2] = [
    b"http://schemas.openxmlformats.org/drawingml/2006/chart",
    b"http://purl.oclc.org/ooxml/drawingml/chart",
];

fn graphic_data_is_chart(e: &BytesStart) -> bool {
    for a in e.attributes().flatten() {
        if a.key == QName(b"uri") && CHART_GRAPHIC_DATA_URIS.contains(&a.value.as_ref()) {
            return true;
        }
    }
    false
}

#[derive(Debug, Default)]
struct ChartPartInfo {
    title: Option<String>,
    chart_type: Option<String>,
    series: Vec<ChartSeries>,
}

#[derive(Debug, Copy, Clone)]
enum SeriesRefKind {
    Categories,
    Values,
    XValues,
    YValues,
}

fn parse_chart_part<RS: Read + Seek>(zip: &mut ZipArchive<RS>, chart_path: &str) -> ChartPartInfo {
    let mut xml = match xml_reader(zip, chart_path) {
        Some(Ok(reader)) => reader,
        Some(Err(e)) => {
            warn!("Failed to open chart part {chart_path}: {e}");
            return ChartPartInfo::default();
        }
        None => return ChartPartInfo::default(),
    };

    let mut chart_type: Option<String> = None;

    let mut chart_title_depth: i32 = 0;
    let mut axis_title_depth: i32 = 0;
    let mut in_a_t = false;
    let mut title_buf = String::new();

    let mut series: Vec<ChartSeries> = Vec::new();
    let mut current_series: Option<ChartSeries> = None;
    let mut current_ref_kind: Option<SeriesRefKind> = None;
    let mut in_ref_f = false;
    let mut ref_text_buf = String::new();
    let mut ser_tx_depth: i32 = 0;
    let mut in_ser_tx_v = false;
    let mut ser_name_buf = String::new();

    let mut buf = Vec::with_capacity(1024);
    loop {
        buf.clear();
        let event = xml.read_event_into(&mut buf);
        let is_start = matches!(&event, Ok(Event::Start(_)));
        match event {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let name = e.local_name();
                let n = name.as_ref();

                if is_start {
                    match n {
                        b"catAx" | b"valAx" | b"serAx" | b"dateAx" => axis_title_depth += 1,
                        b"title" if axis_title_depth == 0 => chart_title_depth += 1,
                        b"t" if chart_title_depth > 0 => in_a_t = true,
                        _ => {}
                    }
                    if chart_type.is_none() && n.ends_with(b"Chart") {
                        chart_type = Some(String::from_utf8_lossy(n).into_owned());
                    }
                }

                match n {
                    b"ser" if is_start => {
                        current_series = Some(ChartSeries::default());
                        ser_tx_depth = 0;
                        in_ser_tx_v = false;
                        ser_name_buf.clear();
                    }
                    b"tx" if is_start && current_series.is_some() => {
                        ser_tx_depth += 1;
                    }
                    b"v" if is_start && ser_tx_depth > 0 => {
                        in_ser_tx_v = true;
                        ser_name_buf.clear();
                    }
                    b"cat" if is_start && current_series.is_some() => {
                        current_ref_kind = Some(SeriesRefKind::Categories);
                    }
                    b"val" if is_start && current_series.is_some() => {
                        current_ref_kind = Some(SeriesRefKind::Values);
                    }
                    b"xVal" if is_start && current_series.is_some() => {
                        current_ref_kind = Some(SeriesRefKind::XValues);
                    }
                    b"yVal" if is_start && current_series.is_some() => {
                        current_ref_kind = Some(SeriesRefKind::YValues);
                    }
                    b"f" if is_start && current_ref_kind.is_some() => {
                        in_ref_f = true;
                        ref_text_buf.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                let s = match t.xml10_content() {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if in_a_t {
                    title_buf.push_str(&s);
                }
                if in_ser_tx_v {
                    ser_name_buf.push_str(&s);
                }
                if in_ref_f {
                    ref_text_buf.push_str(&s);
                }
            }
            Ok(Event::GeneralRef(e)) => {
                if in_a_t {
                    let _ = unescape_entity_to_buffer(&e, &mut title_buf);
                }
                if in_ser_tx_v {
                    let _ = unescape_entity_to_buffer(&e, &mut ser_name_buf);
                }
                if in_ref_f {
                    let _ = unescape_entity_to_buffer(&e, &mut ref_text_buf);
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                let n = name.as_ref();
                match n {
                    b"t" if in_a_t => in_a_t = false,
                    b"title" if chart_title_depth > 0 && axis_title_depth == 0 => {
                        chart_title_depth -= 1;
                    }
                    b"catAx" | b"valAx" | b"serAx" | b"dateAx" => {
                        axis_title_depth = (axis_title_depth - 1).max(0);
                    }
                    b"f" if in_ref_f => {
                        if let (Some(kind), Some(s)) = (current_ref_kind, current_series.as_mut()) {
                            let raw = ref_text_buf.trim();
                            if !raw.is_empty() {
                                match kind {
                                    SeriesRefKind::Categories => {
                                        s.categories_ref = Some(raw.to_string())
                                    }
                                    SeriesRefKind::Values => s.values_ref = Some(raw.to_string()),
                                    SeriesRefKind::XValues => {
                                        s.x_values_ref = Some(raw.to_string())
                                    }
                                    SeriesRefKind::YValues => {
                                        s.y_values_ref = Some(raw.to_string())
                                    }
                                }
                            }
                        }
                        in_ref_f = false;
                        ref_text_buf.clear();
                    }
                    b"cat" | b"val" | b"xVal" | b"yVal" => {
                        current_ref_kind = None;
                    }
                    b"v" if in_ser_tx_v => {
                        in_ser_tx_v = false;
                        if let Some(s) = current_series.as_mut() {
                            let trimmed = ser_name_buf.trim();
                            if !trimmed.is_empty() {
                                s.name = Some(trimmed.to_string());
                            }
                        }
                    }
                    b"tx" if ser_tx_depth > 0 => {
                        ser_tx_depth -= 1;
                    }
                    b"ser" => {
                        if let Some(s) = current_series.take() {
                            series.push(s);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("XML parse error in chart part {chart_path}: {e}");
                break;
            }
            _ => {}
        }
    }

    let title = {
        let trimmed = title_buf.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    ChartPartInfo {
        title,
        chart_type,
        series,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_relative_target_handles_parent_segment() {
        assert_eq!(
            resolve_relative_target("xl/worksheets/sheet2.xml", "../drawings/drawing1.xml"),
            Some("xl/drawings/drawing1.xml".into()),
        );
    }

    #[test]
    fn resolve_relative_target_handles_absolute() {
        assert_eq!(
            resolve_relative_target("xl/worksheets/sheet2.xml", "/xl/drawings/drawing1.xml"),
            Some("xl/drawings/drawing1.xml".into()),
        );
    }

    #[test]
    fn sheet_rels_path_builds_rels_dir() {
        assert_eq!(
            sheet_rels_path("xl/worksheets/sheet2.xml"),
            Some("xl/worksheets/_rels/sheet2.xml.rels".into()),
        );
    }

    #[test]
    fn drawing_rels_path_builds_rels_dir() {
        assert_eq!(
            drawing_rels_path("xl/drawings/drawing1.xml"),
            Some("xl/drawings/_rels/drawing1.xml.rels".into()),
        );
    }

    #[test]
    fn graphic_data_is_chart_matches_transitional_and_strict_namespaces() {
        let transitional = BytesStart::new("a:graphicData").with_attributes([(
            "uri",
            "http://schemas.openxmlformats.org/drawingml/2006/chart",
        )]);
        assert!(graphic_data_is_chart(&transitional));

        let strict = BytesStart::new("a:graphicData")
            .with_attributes([("uri", "http://purl.oclc.org/ooxml/drawingml/chart")]);
        assert!(graphic_data_is_chart(&strict));

        let table = BytesStart::new("a:graphicData").with_attributes([(
            "uri",
            "http://schemas.openxmlformats.org/drawingml/2006/table",
        )]);
        assert!(!graphic_data_is_chart(&table));
    }
}
