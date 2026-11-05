//! V7 design files (IGDS and MicroStation up to version 7): Intergraph ISFF.
//!
//! Words are 16-bit little-endian, longs two words with the high word first, doubles VAX D-float
//! with the most significant word first (`SPEC_v0.1.md` §A.2). Each element starts with a type
//! and level word and its length in words (§A.3); complex elements are followed by their
//! components, which carry the complex bit.

use std::collections::BTreeMap;

use dgn_model::{
    kind_label, BSplineCurve, BSplineSurface, Cell, Cone, Design, Element, Format, Geometry, Model, Reference, SharedCell, SharedCellDefinition,
    Surface, Symbology, Text, Units,
};
use glam::{DMat3, DVec3};

use crate::{knot_vector, latin1, point, quaternion, Result, Skipped, MAX_DEPTH, MAX_ELEMENTS, UNIT};

/// One element: type, level, flags and its words (including the header).
#[derive(Clone, Copy, Debug)]
pub struct RawElement<'a> {
    pub kind: u8,
    pub level: u8,
    /// A component of a complex element.
    pub complex: bool,
    /// Byte offset in the file.
    pub offset: usize,
    pub bytes: &'a [u8],
}

impl RawElement<'_> {
    pub fn words(&self) -> usize { self.bytes.len() / 2 }
    pub fn word(&self, i: usize) -> u16 {
        self.bytes.get(2 * i..2 * i + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    }
    /// 32-bit integer, high word first ("middle-endian").
    pub fn ulong(&self, i: usize) -> u32 { (u32::from(self.word(i)) << 16) | u32::from(self.word(i + 1)) }
    pub fn long(&self, i: usize) -> i32 { self.ulong(i) as i32 }
    /// 32-bit integer, low word first: the longs of shared cells (§A.8).
    pub fn long_le(&self, i: usize) -> i32 { (u32::from(self.word(i)) | (u32::from(self.word(i + 1)) << 16)) as i32 }
    /// VAX D-float, most significant word first.
    pub fn double(&self, i: usize) -> f64 {
        let u = (0..4).fold(0u64, |u, k| (u << 16) | u64::from(self.word(i + k)));
        let exponent = (u >> 55) & 0xFF;
        if exponent == 0 { return 0.0; }
        f64::from_bits((u & (1 << 63)) | ((exponent + 894) << 52) | ((u & ((1 << 55) - 1)) >> 3))
    }
    /// Range corner `at` (0: low, 3: high) in UOR: unsigned longs offset by 2^31 (§A.2).
    pub fn range(&self, at: usize) -> [f64; 3] {
        std::array::from_fn(|k| f64::from(self.ulong(2 + 2 * (at + k))) - 2_147_483_648.0)
    }
    pub fn properties(&self) -> u16 { self.word(16) }
    pub fn class(&self) -> u16 { self.properties() & 0xF }
    /// The H bit: hole, orphan cell, infinite line or disjoint point string.
    pub fn h_bit(&self) -> bool { self.properties() & 0x8000 != 0 }
    pub fn color(&self) -> u8 { (self.word(17) >> 8) as u8 }
    pub fn weight(&self) -> u8 { ((self.word(17) >> 3) & 0x1F) as u8 }
    pub fn style(&self) -> u8 { (self.word(17) & 7) as u8 }
    /// Graphic elements have a display header; types 0, 1, 5, 8, 9, 10 and 66 do not.
    pub fn graphic(&self) -> bool { !matches!(self.kind, 0 | 1 | 5 | 8 | 9 | 10 | 66) && self.words() >= 18 }
    /// Bytes `at..at + len` as text: printable Latin-1, NULs and trailing spaces dropped.
    fn text(&self, at: usize, len: usize) -> String {
        let bytes = self.bytes.get(at..self.bytes.len().min(at + len)).unwrap_or_default();
        latin1(bytes).replace('\0', "").trim_end().to_string()
    }
}

/// The elements of a V7 file in order, without deleted ones. The first must be the design file
/// header (type 9, level 8).
pub fn elements(data: &[u8]) -> Result<Vec<RawElement<'_>>> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset + 4 <= data.len() {
        let first = u16::from_le_bytes([data[offset], data[offset + 1]]);
        if first == 0xFFFF { break; }
        let words = usize::from(u16::from_le_bytes([data[offset + 2], data[offset + 3]])) + 2;
        let end = (offset + 2 * words).min(data.len());
        if first & 0x8000 == 0 {
            out.push(RawElement {
                kind: ((first >> 8) & 0x7F) as u8, level: (first & 0x3F) as u8, complex: first & 0x80 != 0,
                offset, bytes: &data[offset..end],
            });
            if out.len() > MAX_ELEMENTS { return Err("MicroStation DGN: element limit exceeded".into()); }
        }
        offset += 2 * words;
    }
    match out.first() {
        Some(tcb) if tcb.kind == 9 && tcb.level == 8 => Ok(out),
        _ => Err("MicroStation DGN: not a design file (no design file header)".into()),
    }
}

/// Metres per unit for the two-character unit names of the design file header (§A.4).
pub fn metres(unit: &str) -> Option<f64> {
    Some(match unit.to_ascii_uppercase().as_str() {
        "M" | "MT" | "METER" | "METRE" => 1.0,
        "MM" => 0.001,
        "CM" => 0.01,
        "DM" => 0.1,
        "KM" => 1000.0,
        "FT" | "'" | "IF" => 0.3048,
        "SF" | "USFT" | "SFT" => 1200.0 / 3937.0,
        "IN" | "\"" => 0.0254,
        "YD" => 0.9144,
        "MI" => 1609.344,
        _ => return None,
    })
}

/// 256 RGB triples from byte `at` of a colour table element.
pub(crate) fn color_table(bytes: &[u8], at: usize) -> Option<Vec<[u8; 3]>> {
    bytes.get(at..at + 768).map(|table| table.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// Six characters in DEC Radix-50, two words.
pub fn radix50(words: [u16; 2]) -> String {
    const CHARS: &[u8; 40] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ$.%0123456789";
    words.iter().flat_map(|&w| [w / 1600, (w / 40) % 40, w % 40]).map(|c| CHARS.get(usize::from(c)).map_or('?', |&c| c as char))
        .collect::<String>().trim().to_string()
}

/// A 2D rotation angle in radians: one unit is 1/360000 degree.
fn angle(value: i32) -> f64 { (f64::from(value) / 360_000.0).to_radians() }

/// An arc sweep: sign-magnitude according to ISFF, two's complement in some writers; 0 is a full turn.
fn sweep(raw: u32) -> f64 {
    let magnitude = raw & 0x7FFF_FFFF;
    let value = if magnitude > 360 * 360_000 { raw as i32 } else if raw & 0x8000_0000 != 0 { -(magnitude as i32) } else { magnitude as i32 };
    if value == 0 { std::f64::consts::TAU } else { angle(value) }
}

/// The design file header (TCB): dimension, units, global origin; and the colour table.
struct Header {
    three_d: bool,
    origin: DVec3,
    units: Units,
    colors: Option<Vec<[u8; 3]>>,
}

impl Header {
    fn new(elements: &[RawElement]) -> Result<Self> {
        let tcb = elements[0];
        let three_d = tcb.bytes[0] & 0x40 != 0;
        let sub_per_master = f64::from(tcb.long(556));
        let uor_per_sub = f64::from(tcb.long(558));
        let name = |word: usize| -> String {
            tcb.bytes.get(2 * word..2 * word + 2).map(|b| b.iter().filter(|c| c.is_ascii_graphic()).map(|&c| c as char).collect())
                .unwrap_or_default()
        };
        let (master, sub) = (name(560), name(561));
        let stored = sub_per_master > 0.0 && uor_per_sub > 0.0;
        let (uor_per_master, description) = if stored {
            (sub_per_master * uor_per_sub, format!("1 {master} = {sub_per_master} {sub} = {} UOR", sub_per_master * uor_per_sub))
        } else {
            (1.0, "none in the file header (1 UOR = 1 m assumed)".to_string())
        };
        let metres_per_master = metres(&master)
            .or_else(|| metres(&sub).map(|m| m * sub_per_master.max(1.0)))
            .unwrap_or(1.0);
        let origin = DVec3::new(tcb.double(620), tcb.double(624), tcb.double(628));
        if !origin.is_finite() { return Err("MicroStation DGN: invalid global origin".into()); }
        let colors = elements.iter().filter(|e| e.kind == 5 && e.level == 1).find_map(|e| color_table(e.bytes, 38));
        let units = Units {
            metres_per_uor: metres_per_master / uor_per_master,
            metres_per_master,
            metres_per_sub: if stored { metres_per_master / sub_per_master } else { 1.0 },
            master_name: Some(master).filter(|n| !n.is_empty()),
            sub_name: Some(sub).filter(|n| !n.is_empty()),
            stored,
            description,
        };
        Ok(Header { three_d, origin, units, colors })
    }

    /// A point of longs at word `i`.
    fn point(&self, e: &RawElement, i: usize) -> DVec3 {
        DVec3::new(f64::from(e.long(i)), f64::from(e.long(i + 2)), if self.three_d { f64::from(e.long(i + 4)) } else { 0.0 })
    }
    /// A point of doubles at word `i`.
    fn dpoint(&self, e: &RawElement, i: usize) -> DVec3 {
        DVec3::new(e.double(i), e.double(i + 4), if self.three_d { e.double(i + 8) } else { 0.0 })
    }
    fn point_words(&self) -> usize { if self.three_d { 6 } else { 4 } }
    fn dpoint_words(&self) -> usize { if self.three_d { 12 } else { 8 } }
    /// Orientation at word `i`: a 2D angle (one long) or a 3D quaternion (four longs); the columns
    /// are the element's axes in design coordinates.
    fn rotation(&self, e: &RawElement, i: usize) -> DMat3 {
        if self.three_d {
            let [w, x, y, z] = [0, 2, 4, 6].map(|k| f64::from(e.long(i + k)) / UNIT);
            quaternion(w, x, y, z)
        } else {
            DMat3::from_rotation_z(angle(e.long(i)))
        }
    }
    fn rotation_words(&self) -> usize { if self.three_d { 8 } else { 2 } }
}

/// The index after element `i` and its components. Complex headers count `19 + totlength` words
/// from their start; components carry the complex bit. B-spline headers are followed by their
/// knot, boundary, pole and weight elements.
pub fn span(all: &[RawElement], i: usize, depth: usize) -> usize {
    let e = all[i];
    let mut j = i + 1;
    match e.kind {
        2 | 7 | 12 | 14 | 18 | 19 | 34 if depth <= MAX_DEPTH => {
            let end = e.offset + 2 * (19 + usize::from(e.word(18)));
            while j < all.len() && all[j].offset < end && all[j].complex {
                j = span(all, j, depth + 1);
            }
        }
        24 | 27 => while j < all.len() && matches!(all[j].kind, 21 | 25 | 26 | 28) { j += 1 },
        87 => while j < all.len() && all[j].kind == 88 && all[j].complex { j += 1 },
        _ => {}
    }
    j
}

/// Reads a V7 design file.
pub fn read(data: &[u8]) -> Result<Design> {
    let all = elements(data)?;
    let header = Header::new(&all)?;
    let mut reader = Reader { header: &header, skipped: Skipped::default(), definitions: Vec::new(), names: BTreeMap::new() };
    let mut model = Vec::new();
    let mut references = Vec::new();
    let mut i = 0;
    while i < all.len() {
        let e = all[i];
        let next = span(&all, i, 0);
        if (e.kind == 5 && e.level == 9) || (e.kind == 66 && e.level == 5) {
            match reference(&e) {
                Some(r) => references.push(r),
                None => reader.skipped.add("Malformed reference attachment"),
            }
        } else if e.kind == 34 {
            reader.definition(&all, i);
        } else if e.graphic() {
            if let Some(element) = reader.element(&all, i, 0) { model.push(element); }
        }
        i = next;
    }
    let origin = point(header.origin);
    Ok(Design {
        format: Format::V7,
        revision: None,
        application: None,
        three_d: header.three_d,
        units: header.units.clone(),
        global_origin: origin,
        levels: Vec::new(),
        colors: header.colors.clone(),
        models: vec![Model {
            id: 0, name: None, description: None, three_d: header.three_d, hidden: false, sheet: false, units: header.units.clone(), global_origin: origin, elements: model,
        }],
        shared_cells: reader.definitions,
        references,
        skipped: reader.skipped.0,
    })
}

/// A reference file attachment (ISFF type 5 level 9, type 66 level 5): word offsets from the
/// public structure, `file_num` at the high byte of word 51, `fb_opts` at word 52 and the number
/// of clip vertices at word 246.
fn reference(e: &RawElement) -> Option<Reference> {
    if e.words() < 247 { return None; }
    let file_chars = usize::from(e.word(18)).min(65);
    let clip_count = usize::from(e.word(246)).min(e.words().saturating_sub(247) / 4);
    let longs = |i: usize| [f64::from(e.long(i)), f64::from(e.long(i + 2)), f64::from(e.long(i + 4))];
    Some(Reference {
        number: e.bytes[103].into(),
        model: None,
        file_name: e.text(38, file_chars),
        model_name: None,
        logical_name: e.text(2 * 147, usize::from(e.word(146)).min(22)),
        description: e.text(2 * 159, usize::from(e.word(158)).min(42)),
        display: Some(e.word(53) & 0x2 != 0),
        missing: Some(e.word(52) & 0x80 != 0),
        reference_origin: longs(94),
        matrix: std::array::from_fn(|k| e.double(100 + 4 * k)),
        scale: e.double(136),
        master_origin: longs(140),
        clip: (0..clip_count).map(|k| [f64::from(e.long(247 + 4 * k)), f64::from(e.long(249 + 4 * k))]).collect(),
    })
}

struct Reader<'a> {
    header: &'a Header,
    skipped: Skipped,
    /// Shared cell definitions read so far, and the last one of each name.
    definitions: Vec<SharedCellDefinition>,
    names: BTreeMap<String, usize>,
}

/// The name of a shared cell definition or instance: 8 bytes at words 82 to 85, NUL-padded (§A.8).
fn shared_name(e: &RawElement) -> String { e.text(164, 8) }

impl Reader<'_> {
    /// A shared cell definition (type 34, §A.8): its name and its components in definition
    /// coordinates. An instance uses the last definition of its name read before it.
    fn definition(&mut self, all: &[RawElement], i: usize) {
        let e = all[i];
        if e.words() < 86 { self.skipped.add("Malformed element (Shared cell definition)"); return; }
        let name = shared_name(&e);
        let children = self.children(all, i, 0);
        self.names.insert(name.clone(), self.definitions.len());
        self.definitions.push(SharedCellDefinition { name, children });
    }

    /// A shared cell instance (type 35, §A.8): the row-major matrix of VAX doubles at word 38, the
    /// origin as three little-endian longs at word 74 and the definition's name.
    fn shared_cell(&self, e: &RawElement) -> Option<Geometry> {
        if e.words() < 86 { return None; }
        let rows: [f64; 9] = std::array::from_fn(|k| e.double(38 + 4 * k));
        let origin = [74, 76, 78].map(|w| f64::from(e.long_le(w)));
        if !rows.iter().all(|v| v.is_finite()) { return None; }
        let name = shared_name(e);
        Some(Geometry::SharedCell(SharedCell {
            definition: self.names.get(&name).copied(),
            name: Some(name).filter(|n| !n.is_empty()),
            matrix: [[rows[0], rows[1], rows[2]], [rows[3], rows[4], rows[5]], [rows[6], rows[7], rows[8]]],
            origin,
        }))
    }

    /// An element with its display header and `geometry`.
    fn element_of(&self, e: &RawElement, geometry: Geometry) -> Element {
        Element {
            id: e.offset as u64,
            kind: e.kind.into(),
            level: e.level.into(),
            symbology: Symbology { color: e.color().into(), weight: e.weight().into(), style: e.style().into() },
            class: e.class() as u8,
            graphic_group: e.word(14).into(),
            three_d: self.header.three_d,
            hole: e.h_bit(),
            fill: None,
            geometry,
        }
    }

    /// The components of element `i` read as elements.
    fn children(&mut self, all: &[RawElement], i: usize, depth: usize) -> Vec<Element> {
        let end = span(all, i, 0);
        let mut j = i + 1;
        let mut out = Vec::new();
        while j < end {
            out.extend(self.element(all, j, depth + 1));
            j = span(all, j, depth + 1);
        }
        out
    }

    /// Reads element `i` with its components; unsupported and malformed ones are counted.
    fn element(&mut self, all: &[RawElement], i: usize, depth: usize) -> Option<Element> {
        let e = all[i];
        if depth > MAX_DEPTH { self.skipped.add("Too deeply nested element"); return None; }
        let h = self.header;
        let geometry = match e.kind {
            2 => {
                // Cells keep their components in design coordinates.
                let (trans, values, origin) = if h.three_d { (38, 9, 56) } else { (34, 4, 42) };
                let placed = e.words() >= origin + h.point_words();
                Geometry::Cell(Cell {
                    name: Some(radix50([e.word(19), e.word(20)])),
                    origin: placed.then(|| point(h.point(&e, origin))),
                    transform: placed.then(|| (0..values).map(|k| f64::from(e.long(trans + 2 * k)) * 10_000.0 / UNIT).collect()),
                    brep: false,
                    children: self.children(all, i, depth),
                })
            }
            7 => {
                let origin = 29 + h.rotation_words();
                let complete = e.words() >= origin + h.point_words();
                Geometry::TextNode {
                    number: e.word(20).into(),
                    origin: complete.then(|| point(h.point(&e, origin))),
                    line_spacing: complete.then(|| f64::from(e.long(23))),
                    children: self.children(all, i, depth),
                }
            }
            12 => Geometry::Complex { closed: false, children: self.children(all, i, depth) },
            14 => {
                let end = span(all, i, 0);
                let mut children = Vec::new();
                let mut j = i + 1;
                while j < end {
                    match self.curve(all, j) {
                        Some(g) => children.push(self.element_of(&all[j], g)),
                        None => self.skipped.add("Unsupported complex shape component"),
                    }
                    j = span(all, j, depth + 1);
                }
                Geometry::Complex { closed: true, children }
            }
            3 | 4 | 6 | 11 | 15 | 16 | 27 => match self.curve(all, i) {
                Some(g) => g,
                None => { self.skipped.add("Malformed element"); return None; }
            },
            17 => match self.text(&e) {
                Some(g) => g,
                None => { self.skipped.add("Malformed element"); return None; }
            },
            18 | 19 => {
                // Surfaces and solids: the boundary and rule elements with an outline.
                let surface_type = e.bytes.get(40).copied().unwrap_or(0);
                let children = (i + 1..span(all, i, 0)).filter_map(|j| self.curve(all, j).map(|g| self.element_of(&all[j], g))).collect();
                Geometry::Surface(Surface {
                    solid: e.kind == 19, surface_type, revolution: if e.kind == 18 { surface_type == 8 } else { surface_type == 1 },
                    elements_per_boundary: 1, children,
                })
            }
            22 => Geometry::PointString { points: self.vertices(&e).into_iter().map(point).collect(), disjoint: e.h_bit() },
            23 => match self.cone(&e) {
                Some(g) => g,
                None => { self.skipped.add("Malformed element"); return None; }
            },
            24 => match self.bspline_surface(all, i) {
                Some(g) => g,
                None => { self.skipped.add("Malformed B-spline surface"); return None; }
            },
            87 => match raster_image(&e, &all[i + 1..span(all, i, 0)]) {
                Some(g) => g,
                None => { self.skipped.add("Malformed element (Raster)"); return None; }
            },
            35 => match self.shared_cell(&e) {
                Some(g) => g,
                None => { self.skipped.add("Malformed element (Shared cell)"); return None; }
            },
            // A definition among components is not drawn where it stands.
            34 => return None,
            kind => { self.skipped.add(&kind_label(kind.into())); return None; }
        };
        Some(self.element_of(&e, geometry))
    }

    /// Vertices of a line, line string, shape, curve, pole or point string element.
    fn vertices(&self, e: &RawElement) -> Vec<DVec3> {
        let size = self.header.point_words();
        let (first, count) = if e.kind == 3 { (18, 2) } else { (19, usize::from(e.word(18))) };
        let available = e.words().saturating_sub(first) / size;
        (0..count.min(available)).map(|k| self.header.point(e, first + k * size)).collect()
    }

    /// The geometry of a line-like element (lines, line strings, shapes, curves, ellipses, arcs,
    /// B-spline curves); `None` when it has none.
    fn curve(&self, all: &[RawElement], i: usize) -> Option<Geometry> {
        let e = &all[i];
        let h = self.header;
        match e.kind {
            3 | 4 | 6 => {
                let points: Vec<_> = self.vertices(e).into_iter().map(point).collect();
                if points.len() < if e.kind == 3 { 2 } else { 1 } { return None; }
                Some(if e.kind == 3 { Geometry::Line { points } } else { Geometry::LineString { points, closed: e.kind == 6 } })
            }
            11 => {
                let points = self.vertices(e);
                (points.len() >= 6).then(|| Geometry::Curve { points: points.into_iter().map(point).collect() })
            }
            15 | 16 => {
                let (start, sweep_angle, at) = if e.kind == 16 { (angle(e.long(18)), sweep(e.ulong(20)), 22) } else { (0.0, std::f64::consts::TAU, 18) };
                if e.words() < at + 8 + h.rotation_words() + h.dpoint_words() { return None; }
                let (primary, secondary) = (e.double(at), e.double(at + 4));
                let rotation = h.rotation(e, at + 8);
                let center = h.dpoint(e, at + 8 + h.rotation_words());
                Some(Geometry::Ellipse {
                    center: point(center), axis_a: point(rotation.x_axis * primary), axis_b: point(rotation.y_axis * secondary),
                    start, sweep: sweep_angle, closed: e.kind == 15,
                })
            }
            27 => self.bspline_curve(all, i),
            _ => None,
        }
    }

    /// Text: its box from the origin (the lower left) and the size multipliers (§A.6).
    fn text(&self, e: &RawElement) -> Option<Geometry> {
        let h = self.header;
        let rotation = h.rotation(e, 23);
        let origin_at = 23 + h.rotation_words();
        let count_byte = 2 * (origin_at + h.point_words());
        let count = *e.bytes.get(count_byte)?;
        let text = latin1(&e.bytes.iter().skip(count_byte + 2).take(usize::from(count)).copied().collect::<Vec<_>>());
        // The basic character cell is 6 UOR; multipliers are thousandths. They are signed, a
        // negative one mirrors the text (§A.6, v0.3), unless read unsigned the size still fits the
        // element's range (as some writers store large unsigned values).
        let (low, high) = (e.range(0), e.range(3));
        let characters = f64::from(count).max(1.0);
        let (height, flip_y) = multiplier(e.ulong(21), 1.0, high[1] - low[1]);
        let (width, flip_x) = multiplier(e.ulong(19), characters, high[0] - low[0]);
        let sign = |flip: bool| if flip { -1.0 } else { 1.0 };
        Some(Geometry::Text(Text {
            origin: point(h.point(e, origin_at)), x_axis: point(rotation.x_axis * sign(flip_x)), y_axis: point(rotation.y_axis * sign(flip_y)), width, height,
            text: Some(text), font: e.bytes[36].into(), justification: e.bytes[37].into(),
        }))
    }

    fn cone(&self, e: &RawElement) -> Option<Geometry> {
        if e.words() < 59 || !self.header.three_d { return None; }
        let rotation = self.header.rotation(e, 19);
        let center = |at: usize| [e.double(at), e.double(at + 4), e.double(at + 8)];
        Some(Geometry::Cone(Cone {
            centers: [center(27), center(43)],
            radii: [e.double(39), e.double(55)],
            x_axis: point(rotation.x_axis),
            y_axis: point(rotation.y_axis),
            capped: e.word(18) & 0x8000 == 0,
        }))
    }

    /// Pole rows with their weights following a B-spline header: poles from type 21 elements,
    /// weights from the type 28 element after a row.
    fn pole_rows(&self, all: &[RawElement], i: usize) -> Vec<(Vec<DVec3>, Option<Vec<f64>>)> {
        let mut rows: Vec<(Vec<DVec3>, Option<Vec<f64>>)> = Vec::new();
        for e in &all[i + 1..span(all, i, 0)] {
            match e.kind {
                21 => rows.push((self.vertices(e), None)),
                28 => if let Some((poles, weights)) = rows.last_mut() {
                    *weights = Some((0..poles.len()).map(|k| {
                        let weight = f64::from(e.long(18 + 2 * k)) / UNIT;
                        if weight > 0.0 && e.words() >= 20 + 2 * k { weight } else { 1.0 }
                    }).collect());
                },
                _ => {}
            }
        }
        rows
    }

    fn bspline_curve(&self, all: &[RawElement], i: usize) -> Option<Geometry> {
        let (order, closed, rational) = bspline_flags(all[i].word(20));
        let (poles, weights) = self.pole_rows(all, i).into_iter().next()?;
        if poles.len() < 2 { return None; }
        let knot_element = all[i + 1..span(all, i, 0)].iter().find(|c| c.kind == 26);
        let interior = knots(knot_element, poles.len().saturating_sub(order), 0);
        let knots = if poles.len() >= order { knot_vector(poles.len(), order, interior, closed) } else { Vec::new() };
        Some(Geometry::BSplineCurve(BSplineCurve {
            order, closed, periodic: closed, rational, poles: poles.into_iter().map(point).collect(), weights, knots,
        }))
    }

    fn bspline_surface(&self, all: &[RawElement], i: usize) -> Option<Geometry> {
        let e = all[i];
        let (order_u, closed_u, _) = bspline_flags(e.word(20));
        let (order_v, closed_v, _) = bspline_flags(e.word(24));
        let rows = self.pole_rows(all, i);
        let nu = rows.first()?.0.len();
        if nu < order_u || rows.len() < order_v || rows.iter().any(|r| r.0.len() != nu) { return None; }
        let parts = &all[i + 1..span(all, i, 0)];
        let knot_element = parts.iter().find(|c| c.kind == 26);
        let knots_u = knot_vector(nu, order_u, knots(knot_element, nu - order_u, 0), closed_u);
        let knots_v = knot_vector(rows.len(), order_v, knots(knot_element, rows.len() - order_v, nu - order_u), closed_v);
        let weighted = rows.iter().any(|r| r.1.is_some());
        Some(Geometry::BSplineSurface(BSplineSurface {
            order_u, order_v, periodic_u: closed_u, periodic_v: closed_v,
            weights: weighted.then(|| rows.iter().map(|r| r.1.clone().unwrap_or_else(|| vec![1.0; nu])).collect()),
            rows: rows.into_iter().map(|r| r.0.into_iter().map(point).collect()).collect(),
            knots_u, knots_v,
            trimmed: parts.iter().any(|c| c.kind == 25),
        }))
    }
}

/// Interior knots (fractions) from a knot element, or none.
fn knots(element: Option<&RawElement>, count: usize, skip: usize) -> Option<Vec<f64>> {
    let e = element?;
    let values: Vec<f64> = (0..count).map(|k| f64::from(e.long(18 + 2 * (skip + k))) / UNIT).collect();
    (e.words() >= 18 + 2 * (skip + count)).then_some(values)
}

/// A raster (ISFF types 87 and 88): flags at word 20 (format in bits 3 to 7, transparent bit 9),
/// foreground and background at bytes 42 and 43, extents at words 22 and 23, UOR per pixel (a
/// double) at word 30, the origin (upper left, longs) at word 34; each scan line its flags at word
/// 18, its x and y offset at words 20 and 21, its number of pixels at word 22 and the pixel data
/// from word 23.
fn raster_image(header: &RawElement, lines: &[RawElement]) -> Option<Geometry> {
    if header.words() < 40 { return None; }
    let flags = header.word(20);
    let mut image = crate::raster::Image::new(header.word(22).into(), header.word(23).into(), (flags >> 3) as u8 & 0x1F, header.bytes[42], header.bytes[43])?;
    for line in lines.iter().filter(|l| l.words() >= 23) {
        image.row(line.word(21).into(), line.word(20).into(), usize::from(line.word(22)), &line.bytes[46..]);
    }
    let pixel_size = header.double(30);
    if !(pixel_size.is_finite() && pixel_size > 0.0) { return None; }
    let origin = [34, 36, 38].map(|w| f64::from(header.long(w)));
    Some(Geometry::Raster(image.finish(origin, pixel_size, flags & 0x200 != 0)))
}

/// A text size from a size multiplier for `characters` characters, and whether the text is
/// mirrored that way (§A.6, v0.3): read unsigned when it is not negative as signed, or when the
/// unsigned size is at most 1.05 times the range extent plus 2 UOR; otherwise its magnitude as
/// signed, mirrored.
fn multiplier(raw: u32, characters: f64, extent: f64) -> (f64, bool) {
    let unsigned = f64::from(raw) / 1000.0 * 6.0 * characters;
    if (raw as i32) >= 0 || unsigned <= 1.05 * extent + 2.0 { return (unsigned, false); }
    (f64::from((raw as i32).unsigned_abs()) / 1000.0 * 6.0 * characters, true)
}

/// Order, closed and rational flags from a B-spline flags word.
fn bspline_flags(word: u16) -> (usize, bool, bool) {
    (usize::from(word & 0xF) + 2, word & 0x80 != 0, word & 0x40 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::v7::{dfloat, long, Writer};

    #[test]
    fn numbers_decode_as_documented() {
        let mut bytes = Vec::new();
        for w in long(-123_456).iter().chain(dfloat(1.0).iter()).chain(dfloat(-2.5e6).iter()) { bytes.extend(w.to_le_bytes()); }
        let e = RawElement { kind: 3, level: 1, complex: false, offset: 0, bytes: &bytes };
        assert_eq!(e.long(0), -123_456);
        assert_eq!(e.word(2), 0x4080, "1.0 is 4080 0000 0000 0000 in D-float");
        assert_eq!(e.double(2), 1.0);
        assert_eq!(e.double(6), -2.5e6);
        assert_eq!(radix50([1683, 0]), "ABC");
        assert_eq!(sweep(0), std::f64::consts::TAU);
        assert!((sweep(0x8000_0000 | (90 * 360_000)) + std::f64::consts::FRAC_PI_2).abs() < 1e-12, "sign-magnitude");
        assert!((sweep((-90i32 * 360_000) as u32) + std::f64::consts::FRAC_PI_2).abs() < 1e-12, "two's complement");
    }

    #[test]
    fn header_units_and_origin() {
        let w = Writer::new(true, "FT", "TH", 10, 100, [-1000.0, -2000.0, 5.0]);
        let design = read(&w.bytes()).unwrap();
        assert!(design.three_d);
        assert_eq!(design.units.description, "1 FT = 10 TH = 1000 UOR");
        assert!((design.units.metres_per_uor - 0.3048 / 1000.0).abs() < 1e-15);
        assert_eq!(design.units.master_name.as_deref(), Some("FT"));
        assert_eq!(design.global_origin, [-1000.0, -2000.0, 5.0]);
        let empty = Writer::new(false, "", "", 0, 0, [0.0; 3]);
        let design = read(&empty.bytes()).unwrap();
        assert!(!design.units.stored && design.units.metres_per_uor == 1.0);
        assert!(read(b"not a design file").unwrap_err().0.contains("not a design file"));
    }

    #[test]
    fn cells_keep_their_name_origin_and_components() {
        let mut w = Writer::new(false, "M", "MM", 1000, 1, [0.0; 3]);
        let line = w.points(&[[0, 0, 0], [1000, 0, 0]]);
        let mut cell = vec![0u16];
        cell.extend(long(0x0693_0000)); // RADIX-50 "ABC"
        cell.extend([0u16; 5 + 8 + 8]);
        cell.extend(long(5000));
        cell.extend(long(6000));
        cell[0] = (18 + cell.len() + (18 + line.len()) - 19) as u16;
        w.element(2, 3, 0, 0, 0, &cell);
        w.element(3, 4, 2, 0, 0x80, &line);
        let design = read(&w.bytes()).unwrap();
        let Geometry::Cell(cell) = &design.models[0].elements[0].geometry else { panic!() };
        assert_eq!(cell.name.as_deref(), Some("ABC"));
        assert_eq!(cell.origin, Some([5000.0, 6000.0, 0.0]));
        assert_eq!(cell.children.len(), 1);
        assert_eq!((cell.children[0].level, cell.children[0].symbology.color), (4, 2), "components keep their own symbology");
    }

    #[test]
    fn reference_attachments_are_read_from_their_documented_words() {
        let mut w = Writer::new(false, "M", "MM", 1000, 1, [0.0; 3]);
        let mut body = vec![0u16; 247 - 18 + 4 * 2];
        let set_text = |body: &mut Vec<u16>, word: usize, text: &[u8]| {
            for (k, pair) in text.chunks(2).enumerate() { body[word - 18 + k] = u16::from(pair[0]) | u16::from(*pair.get(1).unwrap_or(&0)) << 8; }
        };
        body[0] = 9;
        set_text(&mut body, 19, b"site.dgn\0");
        body[51 - 18] = 7 << 8; // file number in the high byte of word 51
        body[52 - 18] = 0x80; // missing file
        body[53 - 18] = 0x2; // display
        body[94 - 18..96 - 18].copy_from_slice(&long(100));
        body[100 - 18..104 - 18].copy_from_slice(&dfloat(1.0));
        body[136 - 18..140 - 18].copy_from_slice(&dfloat(2.5));
        body[140 - 18..142 - 18].copy_from_slice(&long(-50));
        body[146 - 18] = 4;
        set_text(&mut body, 147, b"SITE");
        body[246 - 18] = 2;
        body[247 - 18..249 - 18].copy_from_slice(&long(10));
        body[253 - 18..255 - 18].copy_from_slice(&long(30));
        w.element(5, 9, 0, 0, 0, &body);
        let design = read(&w.bytes()).unwrap();
        let r = &design.references[0];
        assert_eq!((r.file_name.as_str(), r.logical_name.as_str(), r.number), ("site.dgn", "SITE", 7));
        assert_eq!((r.missing, r.display), (Some(true), Some(true)));
        assert_eq!((r.reference_origin[0], r.matrix[0], r.scale, r.master_origin[0]), (100.0, 1.0, 2.5, -50.0));
        assert_eq!(r.clip, [[10.0, 0.0], [0.0, 30.0]]);
    }
}
