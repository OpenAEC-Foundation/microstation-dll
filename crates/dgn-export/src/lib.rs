//! Writers from the neutral DGN model ([`dgn_model::Design`]):
//!
//! - [`draw`]: the **drawing**, every element as line work (polylines) and faces in world metres,
//!   with its level, colour and weight, and the properties of each pickable element. For CAD
//!   applications and 2D viewers.
//! - [`Drawing::scene`] / [`scene`]: a **triangle scene** for 3D viewers: one mesh per level,
//!   lines as thin ribbons (flat in 2D designs, two crossed ribbons in 3D designs, as wide as the
//!   line weight on a scale set by the drawing extent), closed planar elements of 3D designs,
//!   cones, surfaces and solids as faces; each top-level element (a whole cell, shared cell, text
//!   node or complex chain) one pickable element.
//! - [`json`]: the model, a summary and the drawing as JSON.
//!
//! ```no_run
//! let design = dgn_read::read_file("drawing.dgn").unwrap();
//! let drawing = dgn_export::draw(&design);
//! for prim in &drawing.prims {
//!     if let dgn_export::Shape::Stroke(points) = &prim.shape { println!("{} points on level {}", points.len(), prim.level); }
//! }
//! let scene = drawing.scene().unwrap();
//! # let _ = scene;
//! ```

use std::collections::{BTreeMap, BTreeSet};

use dgn_model::{kind_name, Design, Element, Format, Geometry, Model, Point, BY_LEVEL_COLOR, BY_LEVEL_STYLE, BY_LEVEL_WEIGHT};
use glam::{DMat3, DVec3};
use serde::Serialize;
use serde_json::{json, Value};

mod geometry;
pub mod json;

use geometry::{akima, bspline_points, bspline_triangles, ellipse, ink, line_half_width, ribbons, triangulate, v};

/// Triangles in a scene at most.
pub const MAX_TRIANGLES: usize = 4_000_000;
/// Components nested deeper than this are not drawn.
const MAX_DEPTH: usize = 16;
/// Elements drawn at most, counting each placement of a shared cell definition again.
const MAX_VISITS: usize = 8_000_000;

/// A shape in world metres.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Shape {
    /// A polyline; one point is a point marker.
    Stroke(Vec<Point>),
    Faces(Vec<[Point; 3]>),
}

/// Geometry of (a part of) one element in world metres.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Prim {
    /// The pickable top-level element this belongs to.
    pub id: u64,
    /// The level it is drawn on.
    pub level: u32,
    pub color: [u8; 3],
    /// Line weight 0 to 31.
    pub weight: u8,
    pub shape: Shape,
}

/// What a design draws, in world metres.
#[derive(Debug, Clone)]
pub struct Drawing {
    pub prims: Vec<Prim>,
    /// Properties of each drawn top-level element: id, level and a flat JSON object.
    pub props: Vec<(u64, u32, Value)>,
    /// Drawn top-level elements by type name.
    pub counts: BTreeMap<&'static str, usize>,
    /// Elements not read or not drawn, by what they are.
    pub skipped: BTreeMap<String, usize>,
    /// Layer names by level.
    pub level_names: BTreeMap<u32, String>,
    pub three_d: bool,
    format: Format,
    units: String,
    metres_per_uor: f64,
    origin: Point,
    references: Vec<String>,
    /// Other facts: models, revision, application.
    extra: Vec<(String, String)>,
    /// Points kept so far, to stop before shared cells multiply a drawing past the triangle limit.
    points: usize,
}

/// One mesh per level.
#[derive(Debug, Clone, Default)]
pub struct SceneLayer {
    pub level: u32,
    pub name: String,
    /// xyz per vertex in metres from [`Scene::world_origin`], Z up.
    pub positions: Vec<f32>,
    /// xyz per vertex, unit length.
    pub normals: Vec<f32>,
    /// sRGB per vertex.
    pub colors: Vec<[u8; 4]>,
    /// Triangle list.
    pub indices: Vec<u32>,
    /// Pickable elements: id, first index and index count in `indices`, sorted by first index.
    pub elements: Vec<(u64, u32, u32)>,
    /// Properties of the elements as flat JSON objects, with the first layer that holds them.
    pub props: Vec<(u64, String)>,
}

/// A triangle scene for viewers.
#[derive(Debug, Clone)]
pub struct Scene {
    /// Layers by level, without empty ones.
    pub layers: Vec<SceneLayer>,
    /// The world point (metres) at the scene's origin: the first point drawn.
    pub world_origin: Point,
    /// Facts about the design for a project information panel.
    pub info: Vec<(String, String)>,
}

/// Draws the default model of a design.
pub fn draw(design: &Design) -> Drawing { draw_model(design, 0) }

/// The name of a model for lists: its name, or `#id`.
fn model_label(model: &Model) -> String {
    let name = model.name.clone().unwrap_or_else(|| format!("#{}", model.id));
    match (model.sheet, model.hidden) {
        (true, true) => format!("{name} (sheet, hidden)"),
        (true, false) => format!("{name} (sheet)"),
        (false, true) => format!("{name} (hidden)"),
        (false, false) => name,
    }
}

/// A reference attachment for lists: the file, or the attached model of this file, and the
/// logical name.
fn reference_label(r: &dgn_model::Reference) -> String {
    let target = if r.file_name.is_empty() { format!("model {}", r.model_name.as_deref().unwrap_or("?")) } else { r.file_name.clone() };
    if r.logical_name.is_empty() { target } else { format!("{target} ({})", r.logical_name) }
}

/// Draws model `index` of a design (0 is the default model).
pub fn draw_model(design: &Design, index: usize) -> Drawing {
    let model = design.models.get(index);
    let (units, origin) = model.map_or((&design.units, design.global_origin), |m| (&m.units, m.global_origin));
    let mut extra = Vec::new();
    if design.models.len() > 1 {
        let names: Vec<String> = design.models.iter().map(model_label).collect();
        extra.push(("DGN models".into(), format!("{}: {}", names.len(), names.join(", "))));
        if let Some(m) = model { extra.push(("DGN model shown".into(), model_label(m))); }
    }
    if let Some(revision) = design.revision_label() { extra.push(("DGN file revision".into(), revision)); }
    if let Some(application) = &design.application { extra.push(("DGN application".into(), application.clone())); }
    let mut drawer = Drawer {
        design,
        metres_per_uor: units.metres_per_uor,
        master_per_uor: units.metres_per_uor / units.metres_per_master,
        origin: v(origin),
        out: Drawing {
            prims: Vec::new(), props: Vec::new(), counts: BTreeMap::new(), skipped: design.skipped.clone(),
            level_names: design.levels.iter().filter_map(|l| l.name.clone().map(|n| (l.id, n))).collect(),
            three_d: model.map_or(design.three_d, |m| m.three_d), format: design.format, units: units.description.clone(),
            metres_per_uor: units.metres_per_uor, origin,
            references: design.references.iter().filter(|r| r.model.is_none_or(|m| model.is_some_and(|model| model.id == m)))
                .map(reference_label).collect(),
            extra,
            points: 0,
        },
        next_id: 0, placement: (DMat3::IDENTITY, DVec3::ZERO), instance_level: None, active: Vec::new(), visits: 0, measures: Vec::new(), cell_fill: None,
        models: model.map(|m| vec![m.id]).unwrap_or_default(), reference: None,
    };
    if let Some(model) = model {
        for e in &model.elements { drawer.top_level(e); }
        drawer.references(model);
    }
    drawer.out
}

/// Draws a design and builds its triangle scene.
pub fn scene(design: &Design) -> Result<Scene, String> { draw(design).scene() }

impl Drawing {
    /// The drawing has outgrown the triangle limit.
    pub fn full(&self) -> bool { self.points > 3 * MAX_TRIANGLES }

    fn skip(&mut self, what: &str) {
        match self.skipped.get_mut(what) {
            Some(count) => *count += 1,
            None => { self.skipped.insert(what.to_string(), 1); }
        }
    }

    /// Adds a shape in design coordinates, which `world` maps to metres; empty and non-finite
    /// shapes are dropped.
    fn push(&mut self, id: u64, level: u32, color: [u8; 3], weight: u8, shape: DShape, world: impl Fn(DVec3) -> DVec3) {
        let shape = match shape {
            DShape::Stroke(points) => Shape::Stroke(points.into_iter().map(|p| world(p).to_array()).collect()),
            DShape::Faces(triangles) => Shape::Faces(triangles.into_iter().map(|t| t.map(|p| world(p).to_array())).collect()),
        };
        let finite = |p: &Point| p.iter().all(|c| c.is_finite());
        let count = match &shape {
            Shape::Stroke(points) if !points.is_empty() && points.iter().all(finite) => points.len(),
            Shape::Faces(triangles) if !triangles.is_empty() && triangles.iter().flatten().all(finite) => 3 * triangles.len(),
            _ => return,
        };
        self.points += count;
        if !self.full() { self.prims.push(Prim { id, level, color, weight, shape }); }
    }

    /// The first point drawn, in world metres: the origin of the scene.
    pub fn world_origin(&self) -> Point {
        self.prims.iter().find_map(|prim| match &prim.shape {
            Shape::Stroke(points) => points.first().copied(),
            Shape::Faces(triangles) => triangles.first().map(|t| t[0]),
        }).unwrap_or([0.0; 3])
    }

    fn list<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
        map.iter().map(|(name, count)| format!("{name}: {count}")).collect::<Vec<_>>().join(", ")
    }

    /// Facts about the design: version, units, origins, drawn and skipped elements.
    pub fn info(&self) -> Vec<(String, String)> {
        let skipped = Self::list(&self.skipped);
        let world = self.world_origin();
        let dimension = if self.three_d { "3D" } else { "2D" };
        let mut info = vec![
            ("DGN version".into(), match self.format { Format::V7 => format!("V7 (ISFF), {dimension}"), Format::V8 => format!("V8, {dimension}") }),
            ("DGN units".into(), self.units.clone()),
            ("DGN metres per UOR".into(), self.metres_per_uor.to_string()),
            ("DGN global origin (UOR)".into(), format!("{}, {}, {}", self.origin[0], self.origin[1], self.origin[2])),
            ("DGN world origin (metres)".into(), format!("{}, {}, {}", world[0], world[1], world[2])),
            ("DGN elements".into(), Self::list(&self.counts)),
            ("DGN skipped elements".into(), if skipped.is_empty() { "0".into() } else { skipped }),
        ];
        if !self.references.is_empty() { info.push(("DGN reference files".into(), self.references.join("; "))); }
        info.extend(self.extra.iter().cloned());
        info
    }

    /// The triangle scene: one layer per level, lines as ribbons.
    pub fn scene(&self) -> Result<Scene, String> {
        if self.full() { return Err("MicroStation DGN: triangle limit exceeded".into()); }
        if self.prims.is_empty() {
            let skipped = Self::list(&self.skipped);
            return Err(format!("MicroStation DGN: no drawable elements. Skipped: {}", if skipped.is_empty() { "none" } else { &skipped }));
        }
        let all: Vec<DVec3> = self.prims.iter().flat_map(|prim| match &prim.shape {
            Shape::Stroke(points) => points.iter().map(|&p| v(p)).collect::<Vec<_>>(),
            Shape::Faces(triangles) => triangles.iter().flatten().map(|&p| v(p)).collect(),
        }).collect();
        let half_width = line_half_width(&all);
        drop(all);
        let origin = v(self.world_origin());
        let mut layers: BTreeMap<u32, SceneLayer> = BTreeMap::new();
        let mut triangles = 0usize;
        for prim in &self.prims {
            let (geometry, color) = match &prim.shape {
                Shape::Stroke(points) => {
                    let points: Vec<DVec3> = points.iter().map(|&p| v(p)).collect();
                    (ribbons(&points, half_width * (1.0 + 0.35 * f64::from(prim.weight)), self.three_d), ink(prim.color))
                }
                Shape::Faces(faces) => (faces.iter().filter_map(|t| {
                    let t = t.map(v);
                    let normal = (t[1] - t[0]).cross(t[2] - t[0]);
                    (normal.length_squared() > 1e-24).then(|| (t, normal.normalize()))
                }).collect(), prim.color),
            };
            triangles += geometry.len();
            if triangles > MAX_TRIANGLES { return Err("MicroStation DGN: triangle limit exceeded".into()); }
            let layer = layers.entry(prim.level).or_insert_with(|| SceneLayer {
                level: prim.level,
                name: self.level_names.get(&prim.level).cloned().unwrap_or_else(|| format!("Level {}", prim.level)),
                ..SceneLayer::default()
            });
            let first = u32::try_from(layer.indices.len()).map_err(|_| "MicroStation DGN: layer index limit exceeded")?;
            for (triangle, normal) in geometry {
                for p in triangle {
                    let local = (p - origin).as_vec3();
                    if !local.is_finite() { return Err("MicroStation DGN: model extent exceeds supported coordinates".into()); }
                    layer.indices.push(u32::try_from(layer.positions.len() / 3).map_err(|_| "MicroStation DGN: layer vertex limit exceeded")?);
                    layer.positions.extend(local.to_array());
                    layer.normals.extend(normal.as_vec3().to_array());
                    layer.colors.push([color[0], color[1], color[2], 255]);
                }
            }
            let count = layer.indices.len() as u32 - first;
            match layer.elements.last_mut() {
                Some(last) if last.0 == prim.id && last.1 + last.2 == first => last.2 += count,
                _ if count > 0 => layer.elements.push((prim.id, first, count)),
                _ => {}
            }
        }
        // Properties go with the first layer that holds the element.
        let mut placed = BTreeSet::new();
        for (id, level, props) in &self.props {
            let target = if layers.get(level).is_some_and(|l| l.elements.iter().any(|e| e.0 == *id)) { Some(*level) }
                else { layers.iter().find(|(_, l)| l.elements.iter().any(|e| e.0 == *id)).map(|(&k, _)| k) };
            if let Some(level) = target.filter(|_| placed.insert(*id)) {
                layers.get_mut(&level).unwrap().props.push((*id, props.to_string()));
            }
        }
        Ok(Scene {
            layers: layers.into_values().filter(|l| !l.indices.is_empty()).collect(),
            world_origin: origin.to_array(),
            info: self.info(),
        })
    }
}

/// A shape in design coordinates while drawing.
enum DShape {
    Stroke(Vec<DVec3>),
    Faces(Vec<[DVec3; 3]>),
}

/// A closed outline: faces when `filled` (closed elements of 3D designs that are not holes) and
/// it triangulates, a closed polyline otherwise.
fn closed_shape(mut points: Vec<DVec3>, filled: bool) -> DShape {
    if points.len() > 1 && points.first() != points.last() { points.push(points[0]); }
    if filled {
        let triangles = triangulate(&points);
        if !triangles.is_empty() { return DShape::Faces(triangles); }
    }
    DShape::Stroke(points)
}

/// The box of a text, from its origin (the lower left) along its axes.
fn text_box(origin: DVec3, x: DVec3, y: DVec3) -> Vec<DVec3> {
    vec![origin, origin + x, origin + x + y, origin + y, origin]
}

/// The points (design coordinates) of a line-like geometry, and whether it is closed; complex
/// chains and shapes join the points of their components.
pub(crate) fn curve_points(geometry: &Geometry) -> Option<(Vec<DVec3>, bool)> {
    let points = |p: &[Point]| p.iter().map(|&p| v(p)).collect::<Vec<_>>();
    match geometry {
        Geometry::Complex { closed, children } => {
            let joined: Vec<DVec3> = children.iter().filter_map(|c| curve_points(&c.geometry)).flat_map(|(points, _)| points).collect();
            (!joined.is_empty()).then_some((joined, *closed))
        }
        Geometry::Line { points: p } => Some((points(p), false)),
        Geometry::LineString { points: p, closed } => Some((points(p), *closed)),
        Geometry::Curve { points: p } => Some((akima(&points(p)), false)),
        Geometry::Ellipse { center, axis_a, axis_b, start, sweep, closed } => Some((ellipse(v(*center), v(*axis_a), v(*axis_b), *start, *sweep), *closed)),
        Geometry::BSplineCurve(curve) => Some((bspline_points(curve), false)),
        _ => None,
    }
}

struct Drawer<'a> {
    design: &'a Design,
    /// The models being drawn, the shown one first, then those attached by reference.
    models: Vec<u32>,
    /// The reference attachment whose model is being drawn.
    reference: Option<String>,
    /// Units and global origin of the model being drawn.
    metres_per_uor: f64,
    master_per_uor: f64,
    origin: DVec3,
    out: Drawing,
    next_id: u64,
    /// Placement of the shared cell definition being drawn: design point = matrix · point + offset.
    placement: (DMat3, DVec3),
    /// The level of the shared cell being drawn, for definition components on level 0.
    instance_level: Option<u32>,
    /// The shared cell definitions being drawn, to stop a definition that holds itself.
    active: Vec<usize>,
    visits: usize,
    /// Distances measured by the dimensions of the top-level element being drawn, in master units.
    measures: Vec<f64>,
    /// The fill colour of the cell being drawn, for its closed components without their own.
    cell_fill: Option<u32>,
}

impl Drawer<'_> {
    fn level(&self, e: &Element) -> u32 {
        match (e.level, self.instance_level) { (0, Some(level)) => level, (level, _) => level }
    }

    /// Draws a top-level element with its components as one pickable element.
    fn top_level(&mut self, e: &Element) {
        self.next_id += 1;
        let id = self.next_id;
        let before = self.out.prims.len();
        let mut texts = Vec::new();
        self.measures.clear();
        self.draw(e, id, 0, &mut texts);
        if self.out.prims.len() == before { return; }
        let props = self.props(e, &texts);
        *self.out.counts.entry(kind_name(e.kind)).or_default() += 1;
        self.out.props.push((id, e.level, props));
    }

    /// The properties of a top-level element.
    fn props(&self, e: &Element, texts: &[String]) -> Value {
        let s = e.symbology;
        let cell_name = match &e.geometry {
            Geometry::Cell(c) => c.name.clone(),
            Geometry::SharedCell(c) => c.name.clone(),
            _ => None,
        };
        let mut props = match self.design.format {
            Format::V7 => {
                let mut props = json!({
                    "DGN element": kind_name(e.kind), "Level": e.level, "Colour index": s.color,
                    "Line weight": s.weight, "Line style": s.style, "Class": e.class,
                });
                if e.kind == 2 { props["Cell"] = cell_name.unwrap_or_default().into(); }
                props
            }
            Format::V8 => {
                let level = e.level;
                let resolved = self.design.resolve(s, level);
                let mut props = json!({
                    "DGN element": kind_name(e.kind), "Level": self.design.level_name(level), "Level ID": level,
                    "Colour index": resolved.color, "Line weight": resolved.weight, "Line style": resolved.style, "Class": e.class,
                    "Element ID": e.id,
                });
                let by_level: Vec<&str> = [(s.style == BY_LEVEL_STYLE, "line style"), (s.weight == BY_LEVEL_WEIGHT, "line weight"), (s.color == BY_LEVEL_COLOR, "colour")]
                    .into_iter().filter_map(|(by_level, what)| by_level.then_some(what)).collect();
                if !by_level.is_empty() { props["By level"] = by_level.join(", ").into(); }
                if let Some(fill) = e.fill { props["Fill colour index"] = self.design.resolve_color(fill, level).into(); }
                if let Some(name) = cell_name { props["Cell"] = name.into(); }
                if matches!(&e.geometry, Geometry::Cell(c) if c.brep) { props["Smart solid"] = "wireframe shown (Parasolid B-rep not read)".into(); }
                props
            }
        };
        if e.graphic_group != 0 { props["Graphic group"] = e.graphic_group.into(); }
        if !texts.is_empty() { props["Text"] = texts.join("\n").into(); }
        if let Some(reference) = &self.reference { props["Reference"] = reference.clone().into(); }
        if !self.measures.is_empty() {
            let values: Vec<String> = self.measures.iter().map(|v| ((v * 1e6).round() / 1e6).to_string()).collect();
            props["Measured (master units)"] = values.join("; ").into();
        }
        props
    }

    fn push(&mut self, e: &Element, id: u64, shape: DShape) { self.push_filled(e, id, shape, None); }

    /// Adds a shape in the element's colour, or in colour index `fill`. Custom line styles are
    /// drawn as solid lines like all others.
    fn push_filled(&mut self, e: &Element, id: u64, shape: DShape, fill: Option<u32>) {
        let (design, (matrix, offset)) = (self.design, self.placement);
        let level = self.level(e);
        let symbology = design.resolve(e.symbology, level);
        let color = fill.map_or(symbology.color, |fill| design.resolve_color(fill, level));
        let (origin, scale) = (self.origin, self.metres_per_uor);
        self.out.push(id, level, design.rgb(color), symbology.weight.min(31) as u8, shape, |p| (matrix * p + offset - origin) * scale);
    }

    /// A closed outline. With a fill (its own or its cell's): faces in the fill colour and the
    /// outline in the element's colour; otherwise faces of closed 3D elements, outlines else.
    fn closed(&mut self, e: &Element, id: u64, mut points: Vec<DVec3>, filled: bool) {
        let Some(fill) = e.fill.or(self.cell_fill) else {
            self.push(e, id, closed_shape(points, filled));
            return;
        };
        if points.len() > 1 && points.first() != points.last() { points.push(points[0]); }
        let faces = triangulate(&points);
        if !faces.is_empty() { self.push_filled(e, id, DShape::Faces(faces), Some(fill)); }
        self.push(e, id, DShape::Stroke(points));
    }

    /// Draws an element (and its components) as part of pickable element `id`.
    fn draw(&mut self, e: &Element, id: u64, depth: usize, texts: &mut Vec<String>) {
        if depth > MAX_DEPTH { self.out.skip("Too deeply nested element"); return; }
        if self.out.full() { return; }
        self.visits += 1;
        if self.visits > MAX_VISITS {
            if self.visits == MAX_VISITS + 1 { self.out.skip("Element limit exceeded (rest skipped)"); }
            return;
        }
        let filled = e.three_d && !e.hole;
        match &e.geometry {
            Geometry::Cell(cell) => {
                // The fill of a filled cell fills its closed components.
                let saved = self.cell_fill;
                self.cell_fill = e.fill.or(saved);
                for c in &cell.children { self.draw(c, id, depth + 1, texts); }
                self.cell_fill = saved;
            }
            Geometry::TextNode { children, .. } | Geometry::Complex { closed: false, children } => {
                for c in children { self.draw(c, id, depth + 1, texts); }
            }
            Geometry::Complex { closed: true, children } => {
                let outline: Vec<DVec3> = children.iter().filter_map(|c| curve_points(&c.geometry)).flat_map(|(points, _)| points).collect();
                self.closed(e, id, outline, filled);
            }
            Geometry::Line { .. } | Geometry::LineString { .. } | Geometry::Curve { .. } | Geometry::Ellipse { .. } | Geometry::BSplineCurve(_) => {
                if let Some((points, closed)) = curve_points(&e.geometry) {
                    if closed && e.kind != 3 { self.closed(e, id, points, filled); } else { self.push(e, id, DShape::Stroke(points)); }
                }
            }
            Geometry::Text(t) => {
                self.push(e, id, DShape::Stroke(text_box(v(t.origin), v(t.x_axis) * t.width, v(t.y_axis) * t.height)));
                if let Some(text) = t.text.as_ref().filter(|t| !t.trim().is_empty()) { texts.push(text.clone()); }
            }
            Geometry::PointString { points, disjoint } => {
                let points: Vec<DVec3> = points.iter().map(|&p| v(p)).collect();
                if *disjoint { for p in points { self.push(e, id, DShape::Stroke(vec![p])); } } else { self.push(e, id, DShape::Stroke(points)); }
            }
            Geometry::Cone(c) => {
                let (x, y) = (v(c.x_axis), v(c.y_axis));
                let circle = |k: usize| ellipse(v(c.centers[k]), x * c.radii[k], y * c.radii[k], 0.0, std::f64::consts::TAU);
                let (bottom, top) = (circle(0), circle(1));
                let mut triangles = Vec::new();
                for k in 0..bottom.len() - 1 {
                    triangles.push([bottom[k], bottom[k + 1], top[k + 1]]);
                    triangles.push([bottom[k], top[k + 1], top[k]]);
                }
                if c.capped {
                    triangles.extend(triangulate(&bottom));
                    triangles.extend(triangulate(&top));
                }
                self.push(e, id, DShape::Faces(triangles));
            }
            Geometry::Surface(s) => self.surface(e, s, id),
            Geometry::BSplineSurface(s) => {
                if s.trimmed { self.out.skip("B-spline surface trim boundary (ignored)"); }
                self.push(e, id, DShape::Faces(bspline_triangles(s)));
            }
            Geometry::Dimension(d) => {
                // Extension lines from each point to its foot, and the dimension line through the feet.
                for (&point, &foot) in d.points.iter().zip(&d.feet) {
                    if point != foot { self.push(e, id, DShape::Stroke(vec![v(point), v(foot)])); }
                }
                self.push(e, id, DShape::Stroke(d.feet.iter().map(|&p| v(p)).collect()));
                let master_per_uor = self.master_per_uor;
                self.measures.extend(d.distances.iter().map(|d| d * master_per_uor));
            }
            Geometry::Mesh(m) => {
                // Each face as a fan of triangles.
                let valid = |face: &&Vec<usize>| face.len() >= 3 && face.iter().all(|&i| i < m.vertices.len());
                let triangles = m.faces.iter().filter(valid).flat_map(|face| {
                    (1..face.len() - 1).map(move |k| [face[0], face[k], face[k + 1]].map(|i| v(m.vertices[i])))
                }).collect();
                self.push(e, id, DShape::Faces(triangles));
            }
            Geometry::SharedCell(s) => self.shared_cell(e, s, id, depth, texts),
            Geometry::Raster(r) => self.raster(e, r, id),
        }
    }

    /// The models of this file attached to `model` by reference (§B.20): their elements placed by
    /// the attachment (`origin in this model + scale · matrix · (point - origin in the attached
    /// model)`), each top-level element pickable. A model attached again inside itself is not
    /// drawn twice.
    fn references(&mut self, model: &Model) {
        let design = self.design;
        for r in design.references.iter().filter(|r| r.model == Some(model.id) && r.file_name.is_empty()) {
            let target = r.model_name.as_ref().and_then(|name| design.models.iter().find(|m| m.name.as_ref() == Some(name)));
            let Some(target) = target else { self.out.skip("Reference to a model that is not in the file"); continue };
            if self.models.contains(&target.id) { self.out.skip("Reference that attaches itself"); continue; }
            let matrix = DMat3::from_cols_array(&r.matrix).transpose() * r.scale;
            if !matrix.is_finite() { self.out.skip("Malformed reference attachment"); continue; }
            let saved = (self.placement, self.reference.take());
            let (outer, offset) = self.placement;
            self.placement = (outer * matrix, outer * (v(r.master_origin) - matrix * v(r.reference_origin)) + offset);
            self.reference = Some(reference_label(r));
            self.models.push(target.id);
            *self.out.counts.entry("Reference model").or_default() += 1;
            for e in &target.elements { self.top_level(e); }
            self.references(target);
            self.models.pop();
            (self.placement, self.reference) = saved;
        }
    }

    /// A raster as faces: each run of equal pixels in a row one rectangle, in the pixel's colour;
    /// transparent pixels left out.
    fn raster(&mut self, e: &Element, r: &dgn_model::Raster, id: u64) {
        let (width, height) = (r.width as usize, r.height as usize);
        if r.pixels.len() != width * height || r.pixel_size.is_nan() || r.pixel_size <= 0.0 { return; }
        let (o, size) = (v(r.origin), r.pixel_size);
        let mut by_color: BTreeMap<u8, Vec<[DVec3; 3]>> = BTreeMap::new();
        for (y, row) in r.pixels.chunks_exact(width).enumerate() {
            let mut x = 0;
            while x < width {
                let color = row[x];
                let end = x + row[x..].iter().take_while(|&&c| c == color).count();
                if Some(color) != r.transparent {
                    let (x0, x1) = (o.x + x as f64 * size, o.x + end as f64 * size);
                    let (y0, y1) = (o.y - (y + 1) as f64 * size, o.y - y as f64 * size);
                    let [a, b, c, d] = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(px, py)| DVec3::new(px, py, o.z));
                    by_color.entry(color).or_default().extend([[a, b, c], [a, c, d]]);
                }
                x = end;
            }
        }
        for (color, faces) in by_color { self.push_filled(e, id, DShape::Faces(faces), Some(color.into())); }
    }

    /// Surfaces and solids of projection: faces between equal boundary outlines, capped for a
    /// solid. Others (surfaces of revolution, unequal boundaries) are drawn as their outlines.
    /// A boundary is a run of boundary elements (class 0) of the stored length, joined.
    fn surface(&mut self, e: &Element, s: &dgn_model::Surface, id: u64) {
        let lines: Vec<(&Element, Vec<DVec3>)> = s.children.iter().filter_map(|c| curve_points(&c.geometry).map(|(points, _)| (c, points))).collect();
        let edges: Vec<&Vec<DVec3>> = lines.iter().filter(|(c, _)| c.class == 0).map(|(_, points)| points).collect();
        let joined: Vec<Vec<DVec3>> = edges.chunks(s.elements_per_boundary.max(1)).map(|run| {
            let mut boundary: Vec<DVec3> = Vec::new();
            for points in run {
                let skip = usize::from(boundary.last().is_some_and(|last| points.first() == Some(last)));
                boundary.extend(&points[skip..]);
            }
            boundary
        }).collect();
        let boundaries: Vec<&Vec<DVec3>> = joined.iter().collect();
        let ruled = !s.revolution && boundaries.len() >= 2 && boundaries.windows(2).all(|pair| pair[0].len() == pair[1].len() && pair[0].len() >= 2);
        if !ruled {
            if s.revolution { self.out.skip("Surface of revolution (outline drawn)"); }
            for (c, points) in &lines { self.push(c, id, DShape::Stroke(points.clone())); }
            return;
        }
        let mut triangles = Vec::new();
        for pair in boundaries.windows(2) {
            for k in 0..pair[0].len() - 1 {
                let (a, b, c, d) = (pair[0][k], pair[0][k + 1], pair[1][k + 1], pair[1][k]);
                triangles.push([a, b, c]);
                triangles.push([a, c, d]);
            }
        }
        if s.solid {
            for cap in [boundaries[0], boundaries[boundaries.len() - 1]] {
                if cap.first() == cap.last() { triangles.extend(triangulate(cap)); }
            }
        }
        self.push(e, id, DShape::Faces(triangles));
    }

    /// A shared cell: the components of its definition, placed by the instance's matrix (stored
    /// row by row) and origin.
    fn shared_cell(&mut self, e: &Element, s: &dgn_model::SharedCell, id: u64, depth: usize, texts: &mut Vec<String>) {
        let design = self.design;
        let Some(definition) = s.definition.filter(|&k| k < design.shared_cells.len()) else {
            self.out.skip("Shared cell without definition");
            return;
        };
        if self.active.contains(&definition) { self.out.skip("Shared cell that contains itself"); return; }
        let matrix = DMat3::from_cols_array(&[s.matrix[0], s.matrix[1], s.matrix[2]].concat().try_into().unwrap()).transpose();
        let saved = (self.placement, self.instance_level);
        let (outer, offset) = self.placement;
        self.placement = (outer * matrix, outer * v(s.origin) + offset);
        self.instance_level = Some(self.level(e));
        self.active.push(definition);
        for c in &design.shared_cells[definition].children { self.draw(c, id, depth + 1, texts); }
        self.active.pop();
        (self.placement, self.instance_level) = saved;
    }
}
