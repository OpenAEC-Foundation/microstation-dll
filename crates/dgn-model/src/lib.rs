//! Neutral model of a MicroStation design file (`.dgn`), as read by `dgn-read`.
//!
//! Coordinates are **design coordinates** in units of resolution (UOR), exactly as stored:
//! [`Design::to_metres`] maps them to world metres, `(p - global origin) * metres per UOR`.
//! Components of cells are in design coordinates; components of shared cell definitions are in
//! definition coordinates and placed by each [`Geometry::SharedCell`].
//!
//! Symbology is kept as stored. In V8 files colour, weight and style may be "by level"
//! ([`BY_LEVEL_COLOR`], [`BY_LEVEL_WEIGHT`], [`BY_LEVEL_STYLE`]); [`Design::resolve`] replaces
//! them with the symbology of the level.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A point or vector `[x, y, z]`; 2D elements have `z = 0`.
pub type Point = [f64; 3];

/// Colour "by level" in a V8 display header.
pub const BY_LEVEL_COLOR: u32 = 0xFFFF_FFFF;
/// Line weight "by level" in a V8 display header.
pub const BY_LEVEL_WEIGHT: u32 = 0xFFFF_FFFF;
/// Line style "by level" in a V8 display header.
pub const BY_LEVEL_STYLE: i32 = 0x7FFF_FFFF;

/// The file format generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    /// Intergraph ISFF: IGDS and MicroStation up to version 7.
    V7,
    /// An OLE2 compound file of zlib-compressed element streams: MicroStation V8, V8i, CONNECT
    /// and later.
    V8,
}

/// A whole design file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Design {
    pub format: Format,
    /// V8: the format revision in the file header (`Dgn~H`): 5 (V8 2004 Edition), 103 and 104
    /// (V8i), 202 (CONNECT Edition and later).
    pub revision: Option<u32>,
    /// V8: the application that wrote the file (OLE summary information), such as
    /// `MicroStation v8.11.7.443`.
    pub application: Option<String>,
    /// A 3D design (V7: the design file header; V8: the default model is 3D).
    pub three_d: bool,
    /// Units of the default model.
    pub units: Units,
    /// The global origin of the default model in UOR.
    pub global_origin: Point,
    /// Levels with a name or symbology in the file, by ascending ID. V7 files have none: their
    /// levels are only numbers (1 to 63).
    pub levels: Vec<Level>,
    /// The stored colour table: 256 RGB slots, slot 0 the background, colour `c` in slot `c + 1`.
    /// `None`: no table in the file (see [`default_color`]).
    pub colors: Option<Vec<[u8; 3]>>,
    /// The models: V7 files have one; V8 files every model, the default model (`#000000`)
    /// first, models the file keeps out of its index (`hidden`) last.
    pub models: Vec<Model>,
    /// Shared cell definitions (V8), placed by [`Geometry::SharedCell`] elements.
    pub shared_cells: Vec<SharedCellDefinition>,
    /// Reference file attachments (V7: ISFF type 5 level 9 and type 66 level 5; V8: type 100).
    pub references: Vec<Reference>,
    /// Elements and parts that were not read, by what they are, with their number.
    pub skipped: BTreeMap<String, usize>,
}

/// Units of the design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Units {
    pub metres_per_uor: f64,
    /// Metres per master unit (1 when the file gives none).
    pub metres_per_master: f64,
    /// Metres per sub unit (1 when the file gives none).
    pub metres_per_sub: f64,
    /// Master and sub unit names where the file stores them (V7: two characters, such as `FT`).
    pub master_name: Option<String>,
    pub sub_name: Option<String>,
    /// The units were read from the file; otherwise 1 UOR = 1 m is assumed.
    pub stored: bool,
    /// A human-readable description, such as `1 FT = 10 TH = 1000 UOR`.
    pub description: String,
}

/// A level (V8 level table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// The level ID elements refer to.
    pub id: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    /// Colour, weight and style of elements drawn "by level".
    pub symbology: Option<Symbology>,
}

/// Colour index, line weight and line style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Symbology {
    /// Index in the colour table (0 to 255); 256 and up are extended colours (V8, not described
    /// yet); [`BY_LEVEL_COLOR`] is the level's colour.
    pub color: u32,
    /// 0 to 31; [`BY_LEVEL_WEIGHT`] is the level's weight.
    pub weight: u32,
    /// 0 to 7 (standard styles); negative values are custom line styles (V8); [`BY_LEVEL_STYLE`]
    /// is the level's style.
    pub style: i32,
}

/// One model: the graphic elements in file order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    /// V8: the model ID (the storage `#NNNNNN`, hexadecimal); V7: 0.
    pub id: u32,
    pub name: Option<String>,
    pub description: Option<String>,
    pub three_d: bool,
    /// V8: a model the file keeps out of its model index (storage `Dgn-Nd`).
    pub hidden: bool,
    pub units: Units,
    /// The global origin in UOR.
    pub global_origin: Point,
    pub elements: Vec<Element>,
}

/// A graphic element with its display header and geometry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// V8: the element ID; V7: the byte offset of the element in the file.
    pub id: u64,
    /// The element type number (3 line, 4 line string, ... as in ISFF).
    pub kind: u16,
    /// V7: the level number (0 to 63); V8: the level ID ([`Level::id`]). In a shared cell
    /// definition, level 0 is the level of the instance.
    pub level: u32,
    pub symbology: Symbology,
    /// Class (0 primary, 1 pattern component, 2 construction, ...).
    pub class: u8,
    pub graphic_group: u32,
    /// Coordinates have three components (V7: a 3D file; V8: per element).
    pub three_d: bool,
    /// The H bit: a hole (shapes), disjoint points (point strings), an infinite line (lines).
    pub hole: bool,
    /// Fill colour of a filled shape or cell (V8 fill linkage); may be [`BY_LEVEL_COLOR`].
    pub fill: Option<u32>,
    pub geometry: Geometry,
}

/// The geometry of an element. Angles are radians.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Geometry {
    /// Type 3: two points.
    Line { points: Vec<Point> },
    /// Types 4 (open) and 6 (shape, closed: the last point repeats the first).
    LineString { points: Vec<Point>, closed: bool },
    /// Type 11: the stored points; the first two and last two only shape the end tangents.
    Curve { points: Vec<Point> },
    /// Type 22: points; `disjoint` points are not connected.
    PointString { points: Vec<Point>, disjoint: bool },
    /// Types 15 (ellipse, `closed`) and 16 (arc): `center + axis_a cos t + axis_b sin t` for
    /// `t` from `start` to `start + sweep`.
    Ellipse { center: Point, axis_a: Point, axis_b: Point, start: f64, sweep: f64, closed: bool },
    /// Type 17.
    Text(Text),
    /// Type 7: a group of text elements.
    TextNode { number: u32, origin: Option<Point>, line_spacing: Option<f64>, children: Vec<Element> },
    /// Types 12 (complex chain) and 14 (complex shape, `closed`): curves joined end to end.
    Complex { closed: bool, children: Vec<Element> },
    /// Type 2: a cell (unshared) with its components in design coordinates.
    Cell(Cell),
    /// Type 35: a placed shared cell. A definition point `p` lies at `origin + matrix * p`.
    SharedCell(SharedCell),
    /// Type 27 with its knot, pole and weight elements.
    BSplineCurve(BSplineCurve),
    /// Type 24 with its knot, pole, weight and boundary elements.
    BSplineSurface(BSplineSurface),
    /// Type 23: a truncated cone between two circles.
    Cone(Cone),
    /// Types 18 (surface) and 19 (solid) of projection or revolution (V7).
    Surface(Surface),
    /// Type 33: a 2D linear dimension (V8).
    Dimension(Dimension),
    /// Type 105 with its arrays (V8): faces between vertices.
    Mesh(Mesh),
}

/// A line of text. Its box runs from `origin` (the lower left) along `x_axis` and `y_axis`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub origin: Point,
    /// Unit vectors of the text's orientation.
    pub x_axis: Point,
    pub y_axis: Point,
    /// Size of the text box in UOR.
    pub width: f64,
    pub height: f64,
    /// The characters; `None` where their encoding is not described yet.
    pub text: Option<String>,
    pub font: u32,
    pub justification: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub name: Option<String>,
    /// V7: the cell origin in design coordinates.
    pub origin: Option<Point>,
    /// V7: the transformation from the cell library definition, as stored (2 x 2 or 3 x 3 values
    /// in ISFF order, the low bit of each long = 4.6566e-6). Informational: the components are
    /// already placed.
    pub transform: Option<Vec<f64>>,
    /// A smart solid or surface (V8): the cell holds the exact Parasolid B-rep, which is not
    /// read; its components are the wireframe.
    pub brep: bool,
    pub children: Vec<Element>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedCell {
    pub name: Option<String>,
    /// Index in [`Design::shared_cells`]; `None` when the file has no definition of that name.
    pub definition: Option<usize>,
    /// Row by row: `world = origin + matrix * definition point`.
    pub matrix: [[f64; 3]; 3],
    pub origin: Point,
}

/// A shared cell definition (type 34; V8: in the non-model elements).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedCellDefinition {
    pub name: String,
    /// The components in definition coordinates.
    pub children: Vec<Element>,
}

/// A B-spline curve as a plain NURBS: evaluated with de Boor on [`BSplineCurve::knots`] over
/// `knots[order - 1] ..= knots[n]`, where `n` counts the poles after a periodic wrap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BSplineCurve {
    pub order: usize,
    /// Closed as stored.
    pub closed: bool,
    /// Uniform periodic: evaluate with the first `order - 1` poles appended again.
    pub periodic: bool,
    pub rational: bool,
    pub poles: Vec<Point>,
    /// One weight per pole (1 where the file gives none or an invalid one).
    pub weights: Option<Vec<f64>>,
    /// The full knot vector (`poles + order` values, or `poles + 2 order - 1` when periodic);
    /// empty with fewer poles than the order (draw the control polygon).
    pub knots: Vec<f64>,
}

/// A B-spline surface: rows of poles along u, one row per v.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BSplineSurface {
    pub order_u: usize,
    pub order_v: usize,
    pub periodic_u: bool,
    pub periodic_v: bool,
    pub rows: Vec<Vec<Point>>,
    pub weights: Option<Vec<Vec<f64>>>,
    pub knots_u: Vec<f64>,
    pub knots_v: Vec<f64>,
    /// The surface has trim boundaries (type 25), which are not applied.
    pub trimmed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cone {
    pub centers: [Point; 2],
    pub radii: [f64; 2],
    /// Unit axes of the circles' plane.
    pub x_axis: Point,
    pub y_axis: Point,
    /// A solid with end caps (not just the surface).
    pub capped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub solid: bool,
    /// The stored surface type (the byte at word 20).
    pub surface_type: u8,
    /// A surface or solid of revolution (otherwise of projection).
    pub revolution: bool,
    /// Successive boundary elements (class 0) of this many form one boundary.
    pub elements_per_boundary: usize,
    /// The boundary (class 0) and rule (class 4) elements, each with its outline.
    pub children: Vec<Element>,
}

/// An indexed mesh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub vertices: Vec<Point>,
    /// Faces as polygons of 0-based vertex indices (at least three each).
    pub faces: Vec<Vec<usize>>,
}

/// A 2D linear dimension: extension lines from `points` to their `feet` on the dimension line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dimension {
    pub points: Vec<Point>,
    pub feet: Vec<Point>,
    /// The dimension's axes: unit length, not always perpendicular (isometric dimensions).
    pub x_axis: Point,
    pub y_axis: Point,
    /// Distance of the dimension line from the first point along `y_axis`, in UOR.
    pub height: f64,
    /// The measured distances between successive points, in UOR.
    pub distances: Vec<f64>,
}

/// A reference file attachment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// V7: the file number; V8: the sequence number of the attachment.
    pub number: u32,
    /// V8: the model the attachment belongs to.
    pub model: Option<u32>,
    /// As stored: may be a full or ProjectWise path; look for the bare file name beside the design.
    pub file_name: String,
    pub logical_name: String,
    pub description: String,
    /// V7: displayed (the display bit of the file displayer options).
    pub display: Option<bool>,
    /// V7: the reference file was missing when the design was last saved.
    pub missing: Option<bool>,
    /// The origin in the reference file, in its UOR.
    pub reference_origin: Point,
    /// Where that origin lies in this design, in UOR.
    pub master_origin: Point,
    /// The transformation from the reference file, nine values as stored (V8: row by row).
    pub matrix: [f64; 9],
    /// Conversion factor from reference UOR to master UOR.
    pub scale: f64,
    /// Clip polygon in master UOR (may contain disconnects for clip masks).
    pub clip: Vec<[f64; 2]>,
}

impl Design {
    /// Design coordinates (UOR) to world metres.
    pub fn to_metres(&self, p: Point) -> Point {
        let k = self.units.metres_per_uor;
        [(p[0] - self.global_origin[0]) * k, (p[1] - self.global_origin[1]) * k, (p[2] - self.global_origin[2]) * k]
    }

    /// Design coordinates (UOR) to master units from the global origin.
    pub fn to_master(&self, p: Point) -> Point {
        let k = self.units.metres_per_uor / self.units.metres_per_master;
        [(p[0] - self.global_origin[0]) * k, (p[1] - self.global_origin[1]) * k, (p[2] - self.global_origin[2]) * k]
    }

    pub fn level(&self, id: u32) -> Option<&Level> {
        self.levels.binary_search_by_key(&id, |l| l.id).ok().map(|k| &self.levels[k])
    }

    /// The level's name, or `Level n`.
    pub fn level_name(&self, id: u32) -> String {
        self.level(id).and_then(|l| l.name.clone()).unwrap_or_else(|| format!("Level {id}"))
    }

    /// Colour index `color` on `level`, the level's colour when it is "by level" (0 for a level
    /// that is not in the table).
    pub fn resolve_color(&self, color: u32, level: u32) -> u32 {
        if color == BY_LEVEL_COLOR { self.level(level).and_then(|l| l.symbology).map_or(0, |s| s.color) } else { color }
    }

    /// Symbology with "by level" values taken from `level` (0 for a level not in the table).
    pub fn resolve(&self, symbology: Symbology, level: u32) -> Symbology {
        let by_level = self.level(level).and_then(|l| l.symbology).unwrap_or_default();
        Symbology {
            color: self.resolve_color(symbology.color, level),
            weight: if symbology.weight == BY_LEVEL_WEIGHT { by_level.weight } else { symbology.weight },
            style: if symbology.style == BY_LEVEL_STYLE { by_level.style } else { symbology.style },
        }
    }

    /// The RGB of a (resolved) colour index: the stored table, or [`default_color`] without one.
    /// Extended colours (256 and up) are not described yet: white.
    pub fn rgb(&self, color: u32) -> [u8; 3] {
        match u8::try_from(color) {
            // Slot 0 of a stored table is the background; colour c is slot c + 1.
            Ok(index) => match &self.colors {
                Some(table) if !table.is_empty() => table[(usize::from(index) + 1) % table.len()],
                _ => default_color(index),
            },
            Err(_) => default_color(0),
        }
    }

    /// Every element of the models and shared cell definitions, components included, depth first.
    pub fn walk(&self, mut visit: impl FnMut(&Element)) {
        fn go(e: &Element, visit: &mut dyn FnMut(&Element)) {
            visit(e);
            for c in e.children() { go(c, visit); }
        }
        for e in self.models.iter().flat_map(|m| &m.elements).chain(self.shared_cells.iter().flat_map(|d| &d.children)) {
            go(e, &mut visit);
        }
    }
}

impl Design {
    /// The default model: the first one.
    pub fn default_model(&self) -> Option<&Model> { self.models.first() }

    /// A label for the file revision: `202 (CONNECT Edition or later)`.
    pub fn revision_label(&self) -> Option<String> {
        self.revision.map(|r| format!("{r} ({})", match r {
            5 => "V8 2004 Edition",
            103 | 104 => "V8i",
            202 => "CONNECT Edition or later",
            r if r > 202 => "newer than CONNECT",
            _ => "unknown",
        }))
    }
}

impl Element {
    /// The components of a complex element.
    pub fn children(&self) -> &[Element] {
        match &self.geometry {
            Geometry::TextNode { children, .. } | Geometry::Complex { children, .. } => children,
            Geometry::Cell(c) => &c.children,
            Geometry::Surface(s) => &s.children,
            _ => &[],
        }
    }
}

/// Our own palette for files without a colour table: the documented first eight colours, then
/// evenly spread hues. (No Bentley table is copied.)
pub fn default_color(index: u8) -> [u8; 3] {
    const FIRST: [[u8; 3]; 8] = [
        [255, 255, 255], [0, 0, 255], [0, 255, 0], [255, 0, 0], [255, 255, 0], [255, 0, 255], [255, 127, 0], [0, 255, 255],
    ];
    if let Some(color) = FIRST.get(usize::from(index)) { return *color; }
    let hue = (f64::from(index) * 137.508) % 360.0;
    let (value, saturation) = (if index % 2 == 0 { 0.85 } else { 0.6 }, 0.75);
    let c = value * saturation;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let (r, g, b) = match (hue / 60.0) as u32 { 0 => (c, x, 0.), 1 => (x, c, 0.), 2 => (0., c, x), 3 => (0., x, c), 4 => (x, 0., c), _ => (c, 0., x) };
    [r, g, b].map(|v| ((v + value - c) * 255.0).round() as u8)
}

/// The name of an element type.
pub fn kind_name(kind: u16) -> &'static str {
    match kind {
        2 => "Cell", 3 => "Line", 4 => "Line string", 6 => "Shape", 7 => "Text node", 11 => "Curve",
        12 => "Complex chain", 14 => "Complex shape", 15 => "Ellipse", 16 => "Arc", 17 => "Text",
        18 => "Surface", 19 => "Solid", 21 => "B-spline poles", 22 => "Point string", 23 => "Cone",
        24 => "B-spline surface", 25 => "B-spline boundary", 26 => "B-spline knots", 27 => "B-spline curve",
        28 => "B-spline weights", 33 => "Dimension", 34 => "Shared cell definition", 35 => "Shared cell",
        36 => "Multi-line", 37 => "Tag", 38 => "Parasolid data", 87 => "Raster header", 88 => "Raster data",
        100 => "Reference attachment", 105 => "Mesh",
        _ => "Unknown element",
    }
}

/// The name of an element type for skipped counts; unknown types by number.
pub fn kind_label(kind: u16) -> String {
    match kind_name(kind) {
        "Unknown element" => format!("Element type {kind}"),
        name => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design() -> Design {
        Design {
            format: Format::V8,
            revision: None,
            application: None,
            three_d: false,
            units: Units { metres_per_uor: 0.001, metres_per_master: 1.0, metres_per_sub: 0.001, master_name: None, sub_name: None, stored: true, description: String::new() },
            global_origin: [1000.0, 0.0, 0.0],
            levels: vec![
                Level { id: 11, name: Some("Grid".into()), description: None, symbology: Some(Symbology { color: 5, weight: 2, style: -71 }) },
                Level { id: 64, name: Some("Default".into()), description: None, symbology: None },
            ],
            colors: None,
            models: Vec::new(),
            shared_cells: Vec::new(),
            references: Vec::new(),
            skipped: BTreeMap::new(),
        }
    }

    #[test]
    fn by_level_symbology_comes_from_the_level() {
        let d = design();
        let by_level = Symbology { color: BY_LEVEL_COLOR, weight: BY_LEVEL_WEIGHT, style: BY_LEVEL_STYLE };
        assert_eq!(d.resolve(by_level, 11), Symbology { color: 5, weight: 2, style: -71 });
        assert_eq!(d.resolve(by_level, 12), Symbology::default());
        assert_eq!(d.resolve(Symbology { color: 3, weight: 1, style: 2 }, 11), Symbology { color: 3, weight: 1, style: 2 });
        assert_eq!(d.level_name(64), "Default");
        assert_eq!(d.level_name(65), "Level 65");
    }

    #[test]
    fn colours_and_units() {
        let mut d = design();
        assert_eq!(d.rgb(3), [255, 0, 0]);
        assert_eq!(d.rgb(300), [255, 255, 255]);
        let mut table = vec![[0u8; 3]; 256];
        table[4] = [1, 2, 3];
        d.colors = Some(table);
        assert_eq!(d.rgb(3), [1, 2, 3], "colour c is slot c + 1");
        assert_eq!(d.to_metres([3000.0, 500.0, 0.0]), [2.0, 0.5, 0.0]);
        assert_eq!(kind_label(107), "Element type 107");
        assert_eq!(kind_label(3), "Line");
    }
}
