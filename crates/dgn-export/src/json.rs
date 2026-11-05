//! JSON views of a design. Non-finite numbers (which a malformed file can hold) become `null`.

use std::collections::BTreeMap;

use dgn_model::{kind_name, Design, Format};
use serde_json::{json, Value};

use crate::{Drawing, Shape};

/// The whole model: units, levels, colours, elements with their geometry, shared cell
/// definitions, reference attachments and skipped counts.
pub fn design(design: &Design) -> Value {
    serde_json::to_value(design).unwrap_or(Value::Null)
}

/// A short description of a design: format, units, origin, levels, element counts by type
/// (components included), references and what was skipped.
pub fn summary(design: &Design) -> Value {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    design.walk(|e| *kinds.entry(kind_name(e.kind).to_string()).or_default() += 1);
    json!({
        "format": match design.format { Format::V7 => "V7", Format::V8 => "V8" },
        "three_d": design.three_d,
        "units": design.units,
        "global_origin": design.global_origin,
        "levels": design.levels.iter().map(|l| json!({"id": l.id, "name": l.name})).collect::<Vec<_>>(),
        "color_table": design.colors.is_some(),
        "models": design.models.iter().map(|m| json!({"name": m.name, "elements": m.elements.len()})).collect::<Vec<_>>(),
        "elements": kinds,
        "shared_cell_definitions": design.shared_cells.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
        "references": design.references.iter().map(|r| json!({"file": r.file_name, "logical_name": r.logical_name, "missing": r.missing})).collect::<Vec<_>>(),
        "skipped": design.skipped,
    })
}

/// The drawing: information rows, element properties and the line work and faces in world
/// metres (`{"id", "level", "color": [r, g, b], "weight", "polyline" | "triangles"}`).
pub fn drawing(drawing: &Drawing) -> Value {
    json!({
        "info": drawing.info().into_iter().collect::<BTreeMap<_, _>>(),
        "levels": drawing.level_names,
        "elements": drawing.props.iter().map(|(id, level, props)| json!({"id": id, "level": level, "properties": props})).collect::<Vec<_>>(),
        "prims": drawing.prims.iter().map(|p| {
            let mut v = json!({"id": p.id, "level": p.level, "color": p.color, "weight": p.weight});
            match &p.shape {
                Shape::Stroke(points) => v["polyline"] = json!(points),
                Shape::Faces(triangles) => v["triangles"] = json!(triangles),
            }
            v
        }).collect::<Vec<_>>(),
    })
}
