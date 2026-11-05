//! V8 design files (MicroStation V8, V8i, CONNECT and later): an OLE2 compound file (§B.1) whose
//! element streams `$1`, `$2`, ... hold zlib data (§B.2). Elements have a little-endian header
//! (§B.3) and IEEE doubles; offsets below are bytes from the start of an element.

use std::collections::BTreeMap;

use dgn_model::{
    kind_label, BSplineCurve, Cell, Design, Dimension, Element, Format, Geometry, Level, Model, SharedCell, SharedCellDefinition, Symbology, Text, Units,
};
use glam::{DMat3, DVec3};

use crate::cfb::{CompoundFile, STORAGE, STREAM};
use crate::{inflate, knot_vector, latin1, le16, le32, le64, lef64, point, quaternion, v7::color_table, Result, Skipped, MAX_DEPTH, MAX_ELEMENTS};

/// The largest decompressed element stream (less in the browser's 32-bit memory).
#[cfg(not(target_arch = "wasm32"))]
const MAX_UNPACKED: usize = 1 << 30;
#[cfg(target_arch = "wasm32")]
const MAX_UNPACKED: usize = 256 << 20;
/// The largest decompressed model header (`Dgn~Mh`, about 4.4 KB).
const MAX_MODEL_HEADER: usize = 64 << 10;
/// ID of a string linkage (§B.7).
pub const STRING_LINKAGE: u16 = 0x56D2;
/// ID of a fill linkage (§B.7).
pub const FILL_LINKAGE: u16 = 0x0041;

/// One V8 element (§B.3).
#[derive(Clone, Copy, Debug)]
pub struct RawElement<'a> {
    pub bytes: &'a [u8],
}

impl<'a> RawElement<'a> {
    pub fn len(&self) -> usize { self.bytes.len() }
    pub fn is_empty(&self) -> bool { self.bytes.is_empty() }
    pub fn u16(&self, at: usize) -> u16 { le16(self.bytes, at) }
    pub fn u32(&self, at: usize) -> u32 { le32(self.bytes, at) }
    pub fn u64(&self, at: usize) -> u64 { le64(self.bytes, at) }
    /// The double at `at`; NaN beyond the element.
    pub fn f64(&self, at: usize) -> f64 { lef64(self.bytes, at) }
    pub fn kind(&self) -> u16 { self.u16(0) }
    pub fn flags(&self) -> u16 { self.u16(2) }
    /// A component of a complex element.
    pub fn component(&self) -> bool { self.flags() & 0x4000 != 0 }
    /// A header with components: flagged, or of a type that always has them.
    pub fn complex(&self) -> bool { self.flags() & 0x2000 != 0 || matches!(self.kind(), 2 | 7 | 12 | 14 | 18 | 19 | 24 | 27 | 34) }
    /// The number of components a header counts (at +0x68), where the specification gives it.
    pub fn component_count(&self) -> Option<u32> { (matches!(self.kind(), 2 | 7 | 12 | 14 | 27) && self.len() >= 0x6C).then(|| self.u32(0x68)) }
    /// The level ID (graphic elements) or kind of table entry (non-graphic elements).
    pub fn level(&self) -> u32 { self.u32(0x0C) }
    pub fn id(&self) -> u64 { self.u64(0x10) }
    pub fn graphic(&self) -> bool {
        self.flags() & 0x0400 == 0 && !matches!(self.kind(), 0 | 1 | 5 | 8 | 9 | 10 | 26 | 28 | 66 | 95..=99) && self.len() >= 0x68
    }
    pub fn properties(&self) -> u32 { self.u32(0x28) }
    /// Two or three coordinates per point: per element in V8.
    pub fn three_d(&self) -> bool { self.properties() & 0x0800 != 0 }
    /// The H bit: hole or disjoint point string.
    pub fn h_bit(&self) -> bool { self.properties() & 0x8000 != 0 }
    pub fn class(&self) -> u32 { self.properties() & 0xF }
    pub fn style(&self) -> u32 { self.u32(0x2C) }
    pub fn weight(&self) -> u32 { self.u32(0x30) }
    pub fn color(&self) -> u32 { self.u32(0x34) }
    /// The stored range: low corner and size, in UOR.
    pub fn range(&self) -> ([i64; 3], [i64; 3]) {
        let v = |at: usize| self.u64(at) as i64;
        ([v(0x38), v(0x40), v(0x48)], [v(0x50), v(0x58), v(0x60)])
    }
    /// The empty range of control elements (§B.12): low x = low y = -2^31, size x = size y = 0.
    pub fn empty_range(&self) -> bool {
        let (low, size) = self.range();
        low[0] == -(1i64 << 31) && low[1] == -(1i64 << 31) && size[0] == 0 && size[1] == 0
    }
    fn point_size(&self) -> usize { if self.three_d() { 24 } else { 16 } }
    /// A point of two or three doubles at byte `at`.
    fn point(&self, at: usize) -> DVec3 {
        DVec3::new(self.f64(at), self.f64(at + 8), if self.three_d() { self.f64(at + 16) } else { 0.0 })
    }
    /// Orientation at byte `at`: a 2D angle in radians or a 3D quaternion of four doubles.
    fn rotation(&self, at: usize) -> DMat3 {
        if self.three_d() { quaternion(self.f64(at), self.f64(at + 8), self.f64(at + 16), self.f64(at + 24)) }
        else { DMat3::from_rotation_z(self.f64(at)) }
    }
    fn rotation_size(&self) -> usize { if self.three_d() { 32 } else { 8 } }
    /// The vertices of a line (two points at +0x68) or of a line string, shape, curve, pole or
    /// point string element (a count at +0x68, points from +0x70).
    fn vertices(&self) -> Vec<DVec3> {
        if self.kind() == 4 && self.flags() & 0x0100 != 0 {
            // A line string with flag 0x0100 (§B.11): a count N at +0x68, a 16-byte marker at
            // +0x70, then N - 1 points of two doubles from +0x80. Its stored range is not valid.
            let count = (self.u32(0x68) as usize).saturating_sub(1).min(self.len().saturating_sub(0x80) / 16);
            return (0..count).map(|k| DVec3::new(self.f64(0x80 + 16 * k), self.f64(0x88 + 16 * k), 0.0)).collect();
        }
        let size = self.point_size();
        let (first, count) = if self.kind() == 3 { (0x68, 2) } else { (0x70, self.u32(0x68) as usize) };
        let available = self.len().saturating_sub(first) / size;
        (0..count.min(available)).map(|k| self.point(first + k * size)).collect()
    }
    /// The linkages behind the element data, from byte `2 × attindx` (§B.7): ID and the whole
    /// linkage including its header.
    pub fn linkages(&self) -> Vec<(u16, &'a [u8])> {
        let bytes: &'a [u8] = self.bytes;
        let mut at = (self.u32(8) as usize).saturating_mul(2);
        let mut out = Vec::new();
        while at.saturating_add(4) <= bytes.len() {
            let words = usize::from(self.u16(at) & 0xFF);
            if words == 0 { break; }
            let end = bytes.len().min(at + 2 * words);
            out.push((self.u16(at + 2), &bytes[at..end]));
            at += 2 * words;
        }
        out
    }
    /// String linkages: key and text.
    pub fn strings(&self) -> Vec<(u32, String)> {
        self.linkages().into_iter().filter(|(id, _)| *id == STRING_LINKAGE).map(|(_, l)| {
            let count = le32(l, 8) as usize;
            (le32(l, 4), linkage_text(l.get(12..l.len().min(12usize.saturating_add(count))).unwrap_or_default()))
        }).collect()
    }
    /// The fill colour of a filled shape or cell: the u32 at +8 of its fill linkage (§B.7).
    pub fn fill_color(&self) -> Option<u32> {
        self.linkages().into_iter().find(|(id, linkage)| *id == FILL_LINKAGE && linkage.len() >= 12).map(|(_, linkage)| le32(linkage, 8))
    }
    /// String linkage `key` (§B.7); empty strings count as none.
    pub fn string(&self, key: u32) -> Option<String> {
        self.strings().into_iter().find(|(k, s)| *k == key && !s.is_empty()).map(|(_, s)| s)
    }
    /// The name of a cell, shared cell or level: string linkage 1; without it, no name.
    pub fn name(&self) -> Option<String> { self.strings().into_iter().find(|(key, _)| *key == 1).map(|(_, name)| name).filter(|name| !name.is_empty()) }
}

/// The text of a string linkage: UTF-16LE after `FF FD`, Latin-1 otherwise; trailing NULs dropped.
fn linkage_text(bytes: &[u8]) -> String {
    let text = match bytes {
        [0xFF, 0xFD, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        _ => latin1(bytes),
    };
    text.trim_end_matches('\0').to_string()
}

/// The characters of a text element (§B.8): Latin-1 after the marker `FF FE 01 00` or without a
/// marker; other markers are not described yet, so their text is not shown (None).
fn text_characters(bytes: &[u8]) -> Option<String> {
    let bytes = match bytes {
        [0xFF, 0xFE, 0x01, 0x00, rest @ ..] => rest,
        [0xFF, 0xFD | 0xFE, ..] => return None,
        _ => bytes,
    };
    Some(latin1(bytes).trim_end_matches('\0').to_string())
}

/// Splits a logical element stream: each element follows a 4-byte prefix and is `2 × wtf` bytes
/// long, wtf being the u32 at +4. Also says whether non-zero bytes were left over.
pub fn split(stream: &[u8]) -> Result<(Vec<RawElement<'_>>, bool)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 12 <= stream.len() {
        let start = at + 4;
        let size = (le32(stream, start + 4) as usize).saturating_mul(2);
        let end = start.saturating_add(size);
        if size < 8 || end > stream.len() { break; }
        out.push(RawElement { bytes: &stream[start..end] });
        if out.len() > MAX_ELEMENTS { return Err("MicroStation DGN: element limit exceeded".into()); }
        at = end;
    }
    Ok((out, stream[at.min(stream.len())..].iter().any(|&b| b != 0)))
}

/// The index after element `i` and its components, at most `end`. A top-level element owns
/// all components that follow it; a nested one as many as its header counts (cells, text nodes,
/// complex chains and shapes, B-spline curves). Other nested headers own nothing: their
/// components are read as their siblings. B-spline headers also own their knot, pole and
/// weight elements.
pub fn span(all: &[RawElement], i: usize, end: usize, depth: usize) -> usize {
    let e = all[i];
    let mut j = i + 1;
    if !e.complex() || depth > MAX_DEPTH { return j; }
    let belongs = |c: &RawElement| c.component() || (e.kind() == 27 && matches!(c.kind(), 21 | 26 | 28));
    if depth == 0 {
        while j < end && belongs(&all[j]) { j += 1; }
    } else if let Some(count) = e.component_count() {
        for _ in 0..count {
            if j >= end || !belongs(&all[j]) { break; }
            j = span(all, j, end, depth + 1);
        }
    }
    j
}

/// The streams of a V8 file the reader uses, inflated.
pub struct Streams {
    /// The graphic elements of the first design model (`Dgn-Md/#000000/Dgn^G`).
    pub graphics: Vec<u8>,
    /// The non-model elements (`Dgn^Nm`): level and colour tables, shared cell definitions.
    pub tables: Vec<u8>,
    /// The model header (`Dgn-Md/#000000/Dgn~Mh`), still compressed.
    pub header: Option<Vec<u8>>,
    /// Models beside the first (storages `Dgn-Md/#00000N`).
    pub further_models: usize,
    /// Streams that could not be inflated completely.
    pub problems: Vec<&'static str>,
}

/// The element streams `$1`, `$2`, ... of a storage, inflated and joined in numeric order: one
/// logical element stream, in which elements run across the stream boundaries (§B.2).
fn element_stream(file: &CompoundFile, storage: Option<usize>, problems: &mut Vec<&'static str>) -> Vec<u8> {
    let Some(storage) = storage else { return Vec::new() };
    let mut parts: Vec<(u64, usize)> = file.children(storage).into_iter().filter_map(|k| {
        let digits = file.entries[k].name.strip_prefix('$')?;
        let number = digits.bytes().all(|b| b.is_ascii_digit()).then(|| digits.parse().ok()).flatten()?;
        (file.entries[k].kind == STREAM).then_some((number, k))
    }).collect();
    parts.sort_unstable();
    let mut out = Vec::new();
    for (_, k) in parts {
        // 16 bytes of header (element counts, version, sequence number), then zlib data; a
        // stream of only the header is empty.
        let block = file.read(k);
        if block.len() <= 16 { continue; }
        if let Err(error) = inflate(&block[16..], &mut out, MAX_UNPACKED) {
            problems.push(error);
            break;
        }
    }
    out
}

/// Opens the compound file of a V8 design and inflates its element streams.
pub fn streams(data: &[u8]) -> Result<Streams> {
    let file = CompoundFile::open(data)?;
    let model = file.path(&["Dgn-Md", "#000000"]).ok_or("MicroStation DGN: this V8 file has no design model (Dgn-Md/#000000)")?;
    // Only the first model is described yet: sheets and further design models are counted.
    let further_models = file.path(&["Dgn-Md"]).map_or(0, |models| {
        file.children(models).into_iter().filter(|&k| k != model && file.entries[k].kind == STORAGE).count()
    });
    let mut problems = Vec::new();
    let graphics = element_stream(&file, file.child(model, "Dgn^G"), &mut problems);
    let tables = element_stream(&file, file.path(&["Dgn^Nm"]), &mut problems);
    let header = file.child(model, "Dgn~Mh").map(|k| file.read(k));
    Ok(Streams { graphics, tables, header, further_models, problems })
}

/// Units and global origin from the model header (§B.5): zlib data of 4096 bytes, a 4-byte
/// prefix and an element of type 66.
fn model_units(packed: Option<&[u8]>) -> Option<(DVec3, Units)> {
    let mut unpacked = Vec::new();
    inflate(packed?, &mut unpacked, MAX_MODEL_HEADER).ok()?;
    let e = RawElement { bytes: unpacked.get(0x1004..)? };
    if e.kind() != 66 { return None; }
    // Units per metre are stored as a numerator and a denominator.
    let per_metre = |at: usize| e.f64(at) / e.f64(at + 8);
    let uor_per_storage = e.f64(0xE0);
    let metres_per_uor = 1.0 / (uor_per_storage * per_metre(0xE8));
    let origin = DVec3::new(e.f64(0x70), e.f64(0x78), e.f64(0x80));
    if !(metres_per_uor.is_finite() && metres_per_uor > 0.0 && origin.is_finite()) { return None; }
    let description = format!(
        "1 master unit = {} m, 1 sub unit = {} m, 1 UOR = {metres_per_uor} m ({uor_per_storage} UOR per storage unit of {} m)",
        1.0 / per_metre(0x50), 1.0 / per_metre(0x60), 1.0 / per_metre(0xE8),
    );
    let valid = |m: f64| Some(m).filter(|m| m.is_finite() && *m > 0.0).unwrap_or(1.0);
    Some((origin, Units {
        metres_per_uor,
        metres_per_master: valid(1.0 / per_metre(0x50)),
        metres_per_sub: valid(1.0 / per_metre(0x60)),
        master_name: None,
        sub_name: None,
        stored: true,
        description,
    }))
}

/// Levels from the level table (§B.6, §B.13): type 95 entries of kind 1, with the ID at +0x20,
/// the name in string linkage 1, the description in 2, and the line style, weight and colour at
/// +0x40, +0x44 and +0x48. The first entry with a value wins.
fn levels(tables: &[RawElement]) -> Vec<Level> {
    let mut levels: BTreeMap<u32, Level> = BTreeMap::new();
    for e in tables.iter().filter(|e| e.kind() == 95 && e.level() == 1) {
        let id = e.u32(0x20);
        let level = levels.entry(id).or_insert(Level { id, name: None, description: None, symbology: None });
        if level.name.is_none() { level.name = e.string(1); }
        if level.description.is_none() { level.description = e.string(2); }
        if level.symbology.is_none() && e.len() >= 0x4C {
            level.symbology = Some(Symbology { style: e.u32(0x40) as i32, weight: e.u32(0x44), color: e.u32(0x48) });
        }
    }
    levels.into_values().collect()
}

/// Reads a V8 design file: the first design model, levels, colours and shared cell definitions.
pub fn read(data: &[u8]) -> Result<Design> {
    let streams = streams(data)?;
    let mut skipped = Skipped::default();
    for _ in 0..streams.further_models { skipped.add("Further model (not read)"); }
    for problem in &streams.problems { skipped.add(problem); }
    let (graphics, broken) = split(&streams.graphics)?;
    if broken { skipped.add("Malformed element stream (rest skipped)"); }
    let (tables, broken) = split(&streams.tables)?;
    if broken { skipped.add("Malformed element stream (rest skipped)"); }
    let (origin, units) = model_units(streams.header.as_deref()).unwrap_or_else(|| (DVec3::ZERO, Units {
        metres_per_uor: 1.0, metres_per_master: 1.0, metres_per_sub: 1.0, master_name: None, sub_name: None, stored: false,
        description: "none in the model header (1 UOR = 1 m assumed)".into(),
    }));
    let colors = tables.iter().filter(|e| e.kind() == 5 && e.level() == 1).find_map(|e| color_table(e.bytes, 0x22));
    let three_d = graphics.iter().any(|e| e.graphic() && e.three_d());

    // Shared cell definitions (type 34 in the non-model elements) by name, the first of a name
    // wins; instances look the name up exactly, then in ASCII lower case.
    let mut found: Vec<(String, usize, usize)> = Vec::new();
    let mut i = 0;
    while i < tables.len() {
        let next = span(&tables, i, tables.len(), 0);
        if tables[i].kind() == 34 {
            if let Some(name) = tables[i].name() {
                if !found.iter().any(|(n, _, _)| *n == name) { found.push((name, i, next)); }
            }
        }
        i = next;
    }
    let mut exact = BTreeMap::new();
    let mut folded = BTreeMap::new();
    for (k, (name, _, _)) in found.iter().enumerate() {
        exact.entry(name.clone()).or_insert(k);
        folded.entry(name.to_ascii_lowercase()).or_insert(k);
    }
    let mut reader = Reader { exact, folded, skipped };
    let shared_cells = found.iter().map(|(name, first, stop)| {
        let mut children = Vec::new();
        let mut j = first + 1;
        while j < *stop {
            children.extend(reader.element(&tables, j, *stop, 1, false));
            j = span(&tables, j, *stop, 1);
        }
        SharedCellDefinition { name: name.clone(), children }
    }).collect();

    let mut elements = Vec::new();
    let mut i = 0;
    while i < graphics.len() {
        let next = span(&graphics, i, graphics.len(), 0);
        if graphics[i].graphic() { elements.extend(reader.element(&graphics, i, graphics.len(), 0, true)); }
        i = next;
    }
    Ok(Design {
        format: Format::V8,
        three_d,
        units,
        global_origin: point(origin),
        levels: levels(&tables),
        colors,
        models: vec![Model { name: None, elements }],
        shared_cells,
        references: Vec::new(),
        skipped: reader.skipped.0,
    })
}

struct Reader {
    /// Shared cell definitions by name, and by name in ASCII lower case.
    exact: BTreeMap<String, usize>,
    folded: BTreeMap<String, usize>,
    skipped: Skipped,
}

impl Reader {
    /// An element whose data is too short or not finite, counted by type.
    fn malformed(&mut self, e: &RawElement) { self.skipped.add(&format!("Malformed element ({})", kind_label(e.kind()))); }

    fn element_of(e: &RawElement, geometry: Geometry) -> Element {
        Element {
            id: e.id(),
            kind: e.kind(),
            level: e.level(),
            symbology: Symbology { color: e.color(), weight: e.weight(), style: e.style() as i32 },
            class: e.class() as u8,
            graphic_group: e.u32(0x20),
            three_d: e.three_d(),
            hole: e.h_bit(),
            fill: e.fill_color(),
            geometry,
        }
    }

    /// Reads element `i` (and its components, which end at most at `end`); unsupported and
    /// malformed ones are counted. `in_model`: an element of the model, not of a definition.
    fn element(&mut self, all: &[RawElement], i: usize, end: usize, depth: usize, in_model: bool) -> Option<Element> {
        let e = all[i];
        if depth > MAX_DEPTH { self.skipped.add("Too deeply nested element"); return None; }
        if !e.graphic() { return None; }
        // Invisible control elements of the model (§B.12), such as "Pattern Control Element"
        // texts at the origin, have the empty range; drawn, they would stretch the drawing.
        if in_model && e.empty_range() { self.skipped.add("Control element"); return None; }
        let stop = span(all, i, end, depth);
        let children = |reader: &mut Reader| {
            let mut out = Vec::new();
            let mut j = i + 1;
            while j < stop {
                out.extend(reader.element(all, j, stop, depth + 1, in_model));
                j = span(all, j, stop, depth + 1);
            }
            out
        };
        let geometry = match e.kind() {
            // Cells keep their components in world coordinates; text nodes hold text elements.
            2 => Geometry::Cell(Cell { name: e.name(), origin: None, transform: None, children: children(self) }),
            7 => Geometry::TextNode {
                number: if e.len() >= 0x70 { e.u32(0x6C) } else { 0 }, origin: None, line_spacing: None, children: children(self),
            },
            12 => Geometry::Complex { closed: false, children: children(self) },
            14 => {
                let mut parts = Vec::new();
                let mut j = i + 1;
                while j < stop {
                    let next = span(all, j, stop, depth + 1);
                    match curve(all, j, next) {
                        Some(g) => parts.push(Self::element_of(&all[j], g)),
                        None => self.skipped.add("Unsupported complex shape component"),
                    }
                    j = next;
                }
                Geometry::Complex { closed: true, children: parts }
            }
            3 | 4 | 6 | 11 | 15 | 16 | 27 => match curve(all, i, stop) {
                Some(g) => g,
                None => { self.malformed(&e); return None; }
            },
            17 => match text(&e) {
                Some(g) => g,
                None => { self.malformed(&e); return None; }
            },
            22 => Geometry::PointString { points: e.vertices().into_iter().map(point).collect(), disjoint: e.h_bit() },
            33 => match dimension(&e) {
                Some(g) => g,
                None => { self.skipped.add("Dimension"); return None; }
            },
            35 => {
                if e.len() < 0x100 { self.malformed(&e); return None; }
                let rows: [f64; 9] = std::array::from_fn(|k| e.f64(0xA0 + 8 * k));
                let origin = [e.f64(0xE8), e.f64(0xF0), e.f64(0xF8)];
                if !rows.iter().chain(&origin).all(|v| v.is_finite()) { self.malformed(&e); return None; }
                let name = e.name();
                let definition = name.as_ref().and_then(|n| self.exact.get(n).or_else(|| self.folded.get(&n.to_ascii_lowercase())).copied());
                Geometry::SharedCell(SharedCell {
                    name, definition,
                    matrix: [[rows[0], rows[1], rows[2]], [rows[3], rows[4], rows[5]], [rows[6], rows[7], rows[8]]],
                    origin,
                })
            }
            kind => { self.skipped.add(&kind_label(kind)); return None; }
        };
        Some(Self::element_of(&e, geometry))
    }
}

/// The geometry of a line-like element whose components end before `stop`; `None` when it has
/// none or its numbers are not finite.
fn curve(all: &[RawElement], i: usize, stop: usize) -> Option<Geometry> {
    let e = all[i];
    let finite = |points: &[DVec3]| points.iter().all(|p| p.is_finite());
    match e.kind() {
        3 | 4 | 6 => {
            let points = e.vertices();
            if points.len() < if e.kind() == 3 { 2 } else { 1 } || !finite(&points) { return None; }
            let points = points.into_iter().map(point).collect();
            Some(if e.kind() == 3 { Geometry::Line { points } } else { Geometry::LineString { points, closed: e.kind() == 6 } })
        }
        11 => {
            let points = e.vertices();
            (points.len() >= 6 && finite(&points)).then(|| Geometry::Curve { points: points.into_iter().map(point).collect() })
        }
        15 | 16 => {
            // Angles are radians; the sign of the sweep is the direction, and 0 a full turn as in V7.
            let arc = e.kind() == 16;
            let (start, sweep, at) = if arc { (e.f64(0x68), e.f64(0x70), 0x78) } else { (0.0, std::f64::consts::TAU, 0x68) };
            let sweep = if sweep == 0.0 { std::f64::consts::TAU } else { sweep };
            let rotation = e.rotation(at + 16);
            let center_at = at + 16 + e.rotation_size();
            if e.len() < center_at + e.point_size() { return None; }
            let (center, a, b) = (e.point(center_at), rotation.x_axis * e.f64(at), rotation.y_axis * e.f64(at + 8));
            if !(finite(&[center, a, b]) && start.is_finite() && sweep.is_finite()) { return None; }
            Some(Geometry::Ellipse { center: point(center), axis_a: point(a), axis_b: point(b), start, sweep, closed: !arc })
        }
        27 => bspline(e, &all[i + 1..stop.max(i + 1)]),
        _ => None,
    }
}

/// A B-spline curve from its header and the knot, pole and weight elements after it (§B.4).
fn bspline(header: RawElement, parts: &[RawElement]) -> Option<Geometry> {
    let flags = header.u32(0x6C);
    let (order, closed, rational) = ((flags & 0xF) as usize + 2, flags & 0x100 != 0, flags & 0x80 != 0);
    let poles = parts.iter().find(|p| p.kind() == 21)?.vertices();
    if poles.len() < 2 || !poles.iter().all(|p| p.is_finite()) { return None; }
    // Knots and weights are doubles from +0x20 to the end of their element.
    let doubles = |kind: u16| parts.iter().find(|p| p.kind() == kind).map(|p| (0..p.len().saturating_sub(0x20) / 8).map(|k| p.f64(0x20 + 8 * k)).collect::<Vec<_>>());
    let n = poles.len();
    let weights = doubles(28).map(|w| (0..n).map(|k| w.get(k).copied().filter(|w| *w > 0.0 && w.is_finite()).unwrap_or(1.0)).collect());
    let ends_meet = poles.first() == poles.last();
    let (periodic, interior) = match doubles(26) {
        // A closed curve whose ends meet (such as the rational circle) is an open, clamped curve
        // with the stored knots strictly between 0 and 1.
        knots if closed && ends_meet => (false, knots.map(|k| k.into_iter().take(n.saturating_sub(1)).filter(|&t| t > 0.0 && t < 1.0).collect())),
        // Other closed curves: uniform periodic.
        _ if closed => (true, None),
        // An open curve stores its interior knots.
        knots => (false, knots.map(|k| k.into_iter().take(n.saturating_sub(order)).collect())),
    };
    let knots = if n >= order { knot_vector(n, order, interior, periodic) } else { Vec::new() };
    Some(Geometry::BSplineCurve(BSplineCurve { order, closed, periodic, rational, poles: poles.into_iter().map(point).collect(), weights, knots }))
}

/// Text (§B.4, §B.8): its box from the origin (the lower left), as wide and high as stored.
fn text(e: &RawElement) -> Option<Geometry> {
    // The number of text bytes is the byte at +0x6E.
    let count = usize::from(e.bytes.get(0x6E).copied().unwrap_or(0));
    let (origin_at, start) = if e.three_d() { (0xB0, 0xCA) } else { (0x98, 0xAA) };
    let bytes = e.bytes.get(start..start + count)?;
    let text = text_characters(bytes);
    // Width and height of the text in UOR at +0x80 and +0x88 (measured on 2D text, assumed for
    // 3D text); without them, as in V7, a character cell of 6 UOR times the multipliers in
    // thousandths.
    let (width, height) = match (e.f64(0x80), e.f64(0x88)) {
        (width, height) if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 => (width, height),
        _ => {
            let characters = text.as_ref().map_or(count, |t| t.chars().count()).max(1);
            (e.f64(0x70) / 1000.0 * 6.0 * characters as f64, e.f64(0x78) / 1000.0 * 6.0)
        }
    };
    let (origin, rotation) = (e.point(origin_at), e.rotation(0x90));
    if !(origin.is_finite() && rotation.is_finite() && width.is_finite() && height.is_finite()) { return None; }
    Some(Geometry::Text(Text {
        origin: point(origin), x_axis: point(rotation.x_axis), y_axis: point(rotation.y_axis), width, height, text,
        font: e.u32(0x68), justification: e.u16(0x6C).into(),
    }))
}

/// A 2D linear dimension (§B.10): the feet of its points on the dimension line and the measured
/// distances. 3D dimensions, short ones and other kinds of dimension (angular, radial,
/// ordinate: a foot more than 1% outside the element's range) are `None`.
fn dimension(e: &RawElement) -> Option<Geometry> {
    let count = usize::from(e.bytes.get(0x87).copied().unwrap_or(0));
    if e.three_d() || count < 2 || e.len() < 0x130 + 0x30 * count { return None; }
    // Points of 0x30 bytes from +0x130; h of the first point is the height of the dimension
    // line above it, along the dimension's y axis.
    let points: Vec<DVec3> = (0..count).map(|i| DVec3::new(e.f64(0x130 + 0x30 * i), e.f64(0x138 + 0x30 * i), 0.0)).collect();
    let height = e.f64(0x130 + 0x18);
    // The columns of the 2 x 2 matrix at +0x110 are the dimension's x and y axes: of length 1,
    // but not always perpendicular (isometric dimensions, such as x = (0.866, 0.5), y = (0, 1)).
    let [m0, m1, m2, m3] = [0, 1, 2, 3].map(|k| e.f64(0x110 + 8 * k));
    let (x, y) = (DVec3::new(m0, m2, 0.0), DVec3::new(m1, m3, 0.0));
    let base = points[0] + y * height;
    // Each point is base + d x + e y: d from the 2 x 2 system, which needs axes that are not
    // parallel. The foot of the point lies at d along the dimension line.
    let determinant = x.x * y.y - x.y * y.x;
    // (NaN fails the comparison too.)
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    if !(determinant.abs() > 1e-9 * x.length() * y.length()) { return None; }
    let along: Vec<f64> = points.iter().map(|&p| {
        let v = p - base;
        (v.x * y.y - v.y * y.x) / determinant
    }).collect();
    let feet: Vec<DVec3> = along.iter().map(|&d| base + x * d).collect();
    // The feet lie in the element's range (+0x38 low corner, +0x50 size, UOR), give or take
    // 1% of its largest size; the points themselves often do not.
    let (low, size) = e.range();
    let (low, size) = (DVec3::from_array(low.map(|v| v as f64)), DVec3::from_array(size.map(|v| v as f64)));
    let margin = 0.01 * size.max_element();
    let inside = |q: &DVec3| (0..2).all(|a| q[a] >= low[a] - margin && q[a] <= low[a] + size[a] + margin);
    if !(points.iter().all(|p| p.is_finite()) && feet.iter().all(|q| q.is_finite() && inside(q))) { return None; }
    Some(Geometry::Dimension(Dimension {
        points: points.into_iter().map(point).collect(),
        feet: feet.into_iter().map(point).collect(),
        x_axis: point(x),
        y_axis: point(y),
        height,
        // The measured value is |d(i+1) - d(i)| times the length of the x axis.
        distances: along.windows(2).map(|pair| (pair[1] - pair[0]).abs() * x.length()).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::v8::*;

    #[test]
    fn nested_text_nodes_own_as_many_texts_as_they_count() {
        // A cell of two components: a text node of one text, then a text of the cell itself.
        let text = text_data([300.0, 300.0], [0.0, 0.0], b"X");
        let mut stream = Vec::new();
        for element in [
            element(2, 0x3000, 1, shown(0, 0), &[2, 0, 0, 0, 0, 0, 0, 0], &[]),
            element(7, 0x7000, 1, shown(0, 0), &[1, 0, 0, 0, 0, 0, 0, 0], &[]),
            element(17, 0x5000, 1, shown(0, 0), &text, &[]),
            element(17, 0x5000, 1, shown(0, 0), &text, &[]),
        ] {
            stream.extend([0; 4]);
            stream.extend(element);
        }
        let (all, broken) = split(&stream).unwrap();
        assert!(!broken);
        assert_eq!(all.len(), 4);
        assert_eq!(span(&all, 1, all.len(), 1), 3, "the nested text node owns one text");
        assert_eq!(span(&all, 0, all.len(), 1), 4, "a nested cell owns its two components");
        assert_eq!(span(&all, 0, all.len(), 0), 4);
    }

    #[test]
    fn linkages_and_text_encodings() {
        let e = element(2, 0x3000, 1, shown(0, 0), &[0; 8], &[(0, "NOT A NAME"), (1, "DOOR"), (2, "")]);
        let e = with_fill(e, 4);
        let raw = RawElement { bytes: &e };
        assert_eq!(raw.name().as_deref(), Some("DOOR"));
        assert_eq!(raw.string(2), None);
        assert_eq!(raw.fill_color(), Some(4));
        assert_eq!(linkage_text(&[0xFF, 0xFD, b'A', 0, 0xE9, 0, 0, 0]), "Aé");
        assert_eq!(text_characters(b"\xFF\xFE\x01\x00myT\xE9xt").as_deref(), Some("myTéxt"));
        assert_eq!(text_characters(b"\xB0C\0").as_deref(), Some("°C"));
        assert_eq!(text_characters(b"\xFF\xFD\x41\x00"), None);
    }

    #[test]
    fn model_header_units() {
        let f = File::new(3048.0, 39370.0, 12000.0, [-2147483647.0, -2147483647.0, 0.0]);
        let (origin, units) = model_units(Some(&f.header)).unwrap();
        assert_eq!(origin.x, -2147483647.0);
        assert!((units.metres_per_uor - 1.0 / (3048.0 * 39370.0 / 12000.0)).abs() < 1e-15, "US survey foot");
        assert_eq!(units.metres_per_sub, 0.001);
        assert!(model_units(Some(b"no zlib")).is_none() && model_units(None).is_none());
    }

    #[test]
    fn levels_take_the_first_name_and_symbology() {
        let tables = [level_with_symbology(11, "Grid", -71, 2, 5), level_entry(11, "Other"), level_entry(64, "Default")];
        let raw: Vec<RawElement> = tables.iter().map(|t| RawElement { bytes: t }).collect();
        let levels = levels(&raw);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].name.as_deref(), Some("Grid"));
        assert_eq!(levels[0].symbology, Some(Symbology { color: 5, weight: 2, style: -71 }));
        assert_eq!((levels[1].id, levels[1].symbology), (64, None));
    }
}
