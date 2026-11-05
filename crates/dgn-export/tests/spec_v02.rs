//! Coverage added with `SPEC_v0.2.md`: models, file revision, 3D primitives, meshes, smart
//! solids, reference attachments (V8) and shared cells (V7). Files are written in the test.

use dgn_export::{Scene, SceneLayer};
use dgn_model::Geometry;
use dgn_read::synth::v7::{dfloat, long, Writer};
use dgn_read::synth::v8::*;

fn scene(design: &dgn_model::Design) -> Scene { dgn_export::scene(design).unwrap() }

fn info<'a>(scene: &'a Scene, key: &str) -> &'a str {
    scene.info.iter().find(|(k, _)| k == key).map_or_else(|| panic!("no {key} in {:?}", scene.info), |(_, v)| v.as_str())
}

fn bounds(layer: &SceneLayer) -> ([f32; 3], [f32; 3]) {
    layer.positions.chunks_exact(3).fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(mut lo, mut hi), p| {
        for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); }
        (lo, hi)
    })
}

fn size(layer: &SceneLayer) -> [f32; 3] {
    let (lo, hi) = bounds(layer);
    [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]
}

fn near(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool { (0..3).all(|k| (a[k] - b[k]).abs() <= tolerance) }

fn triangles(layer: &SceneLayer) -> usize { layer.indices.len() / 3 }

#[test]
fn v8_models_revision_and_application() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    f.header = model_header_stream(1000.0, 1.0, 1.0, [0.0; 3], Some("Default"), true);
    f.revision = Some(202);
    f.application = Some("MicroStation CONNECT 10.16".into());
    f.graphic(element(3, 0x1000, 1, shown(D3, 0), &doubles(&[0.0, 0.0, 0.0, 1000.0, 0.0, 0.0]), &[]));
    // A 2D sheet (ID 0x1a) with a line of 5 m, and a hidden model (Dgn-Nd) with nothing.
    let mut sheet = Vec::new();
    push(&mut sheet, element(3, 0x1000, 1, shown(0, 0), &doubles(&[0.0, 0.0, 5000.0, 0.0]), &[]));
    f.models.push(ExtraModel { id: 0x1a, hidden: false, header: model_header_stream(1000.0, 1.0, 1.0, [0.0; 3], Some("Sheet"), false), graphics: sheet, controls: Vec::new() });
    f.models.push(ExtraModel { id: 2, hidden: true, header: model_header_stream(1000.0, 1.0, 1.0, [0.0; 3], Some("Profile"), false), graphics: Vec::new(), controls: Vec::new() });
    let design = dgn_read::read(&f.bytes()).unwrap();
    let models: Vec<(u32, &str, bool, bool)> = design.models.iter().map(|m| (m.id, m.name.as_deref().unwrap(), m.three_d, m.hidden)).collect();
    assert_eq!(models, [(0, "Default", true, false), (0x1a, "Sheet", false, false), (2, "Profile", false, true)]);
    assert_eq!(design.revision, Some(202));
    assert_eq!(design.units.master_name.as_deref(), Some("m"));
    let s = scene(&design);
    assert_eq!(info(&s, "DGN version"), "V8, 3D");
    assert_eq!(info(&s, "DGN models"), "3: Default, Sheet, Profile (hidden)");
    assert_eq!(info(&s, "DGN model shown"), "Default");
    assert_eq!(info(&s, "DGN file revision"), "202 (CONNECT Edition or later)");
    assert_eq!(info(&s, "DGN application"), "MicroStation CONNECT 10.16");
    assert_eq!(info(&s, "DGN skipped elements"), "0");
    let sheet = dgn_export::draw_model(&design, 1).scene().unwrap();
    assert!((size(&sheet.layers[0])[0] - 5.0).abs() < 0.05, "{:?}", size(&sheet.layers[0]));
    assert!(info(&sheet, "DGN version").ends_with("2D"));
}

#[test]
fn v8_cones_solids_and_bspline_surfaces_are_faces() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    // A cylinder of radius 1 m and height 2 m on level 1.
    f.graphic(cone(1, [0.0, 0.0, 0.0], 1000.0, [0.0, 0.0, 2000.0], 1000.0));
    // A unit cube as a solid on level 2: six components, boundaries of two elements each
    // (+0x6C = 1), a solid of projection (+0x70 = 0), two rule lines (class 4).
    let header: Vec<u8> = [6u32, 1, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
    f.graphic(element(19, 0x3000, 2, shown(D3, 0), &header, &[]));
    for z in [0.0, 1000.0] {
        f.graphic(element(4, 0x5000, 2, shown(D3, 0), &counted(3, &[0.0, 0.0, z, 1000.0, 0.0, z, 1000.0, 1000.0, z]), &[]));
        f.graphic(element(4, 0x5000, 2, shown(D3, 0), &counted(3, &[1000.0, 1000.0, z, 0.0, 1000.0, z, 0.0, 0.0, z]), &[]));
    }
    for x in [0.0, 1000.0] {
        f.graphic(element(3, 0x5000, 2, shown(D3 | 4, 0), &doubles(&[x, 0.0, 0.0, x, 0.0, 1000.0]), &[]));
    }
    // A bilinear B-spline surface of 2 x 2 poles on level 3: the square (10..11, 0..1) m.
    let header: Vec<u8> = [2u32, 0, 2, 0, 0, 0, 2, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
    f.graphic(element(24, 0x3000, 3, shown(D3, 0), &header, &[]));
    for y in [0.0, 1000.0] {
        f.graphic(element(21, 0x5000, 3, shown(D3, 0), &counted(2, &[10_000.0, y, 0.0, 11_000.0, y, 0.0]), &[]));
    }
    let design = dgn_read::read(&f.bytes()).unwrap();
    let Geometry::Surface(solid) = &design.models[0].elements[1].geometry else { panic!() };
    assert_eq!((solid.solid, solid.revolution, solid.elements_per_boundary, solid.children.len()), (true, false, 2, 6));
    let s = scene(&design);
    assert_eq!(info(&s, "DGN elements"), "B-spline surface: 1, Cone: 1, Solid: 1");
    assert_eq!(info(&s, "DGN skipped elements"), "0");
    assert!(near(size(&s.layers[0]), [2.0, 2.0, 2.0], 1e-3), "{:?}", size(&s.layers[0]));
    assert_eq!(triangles(&s.layers[1]), 12, "four sides and two caps of a cube");
    assert!(near(size(&s.layers[1]), [1.0, 1.0, 1.0], 1e-5));
    assert!(near(size(&s.layers[2]), [1.0, 1.0, 0.0], 1e-5));
    assert_eq!(triangles(&s.layers[2]), 2 * 8 * 8);
}

#[test]
fn v8_meshes_are_indexed_faces() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    let v: Vec<[f64; 3]> = (0..8).map(|k| [f64::from(k & 1) * 1000.0, f64::from((k >> 1) & 1) * 1000.0, f64::from(k >> 2) * 1000.0]).collect();
    // Twelve triangles of a cube, 1-based, some edges invisible (negative), each closed by 0;
    // then a face that names vertex 9, which does not exist.
    let faces = [
        [1, 3, 4], [1, 4, 2], [5, 6, 8], [5, 8, 7], [1, 2, 6], [1, 6, 5],
        [3, 7, 8], [3, 8, 4], [1, 5, 7], [1, 7, 3], [2, 4, 8], [2, 8, 6],
    ];
    let mut indices: Vec<i32> = faces.iter().flat_map(|f| [f[0], -f[1], f[2], 0]).collect();
    indices.extend([1, 2, 9, 0]);
    for e in mesh(4, &v, &indices) { f.graphic(e); }
    let design = dgn_read::read(&f.bytes()).unwrap();
    let Geometry::Mesh(m) = &design.models[0].elements[0].geometry else { panic!("{:?}", design.models[0].elements) };
    assert_eq!((m.vertices.len(), m.faces.len()), (8, 12));
    assert_eq!(m.faces[0], [0, 2, 3]);
    let s = scene(&design);
    assert_eq!(info(&s, "DGN elements"), "Mesh: 1");
    assert_eq!(triangles(&s.layers[0]), 12);
    assert!(near(size(&s.layers[0]), [1.0, 1.0, 1.0], 1e-6));
}

#[test]
fn v8_smart_solids_draw_their_wireframe() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    // A cell of an edge and Parasolid data (type 38, no display header: not drawn, not skipped).
    f.graphic(element(2, 0x3000, 5, shown(D3, 0), &[2, 0, 0, 0, 0, 0, 0, 0], &[]));
    f.graphic(element(3, 0x5000, 5, shown(D3, 0), &doubles(&[0.0, 0.0, 0.0, 1000.0, 0.0, 0.0]), &[]));
    let mut parasolid = vec![0u8; 0x18];
    parasolid.extend(b"PS**TRANSMIT FILE created by modeller version 3100");
    f.graphic(element(38, 0x4000, 0, None, &parasolid, &[]));
    // An XAttribute carrier (type 106): not graphic.
    f.graphic(element(106, 0x1000, 5, shown(D3, 0), &[0; 8], &[]));
    let design = dgn_read::read(&f.bytes()).unwrap();
    let Geometry::Cell(cell) = &design.models[0].elements[0].geometry else { panic!() };
    assert!(cell.brep && cell.children.len() == 1);
    let s = scene(&design);
    assert_eq!(info(&s, "DGN elements"), "Cell: 1");
    assert_eq!(info(&s, "DGN skipped elements"), "B-rep (not supported, wireframe shown): 1");
    assert!(s.layers[0].props[0].1.contains("\"Smart solid\""), "{}", s.layers[0].props[0].1);
}

#[test]
fn v8_reference_attachments_are_listed() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    f.control(reference_attachment(1, "PW_WORKDIR:d0105170\\plan.dgn", [5000.0, 0.0, 0.0]));
    f.control(reference_attachment(2, "AUROW.DGN", [0.0; 3]));
    f.graphic(element(3, 0x1000, 1, shown(0, 0), &doubles(&[0.0, 0.0, 1000.0, 0.0]), &[]));
    let design = dgn_read::read(&f.bytes()).unwrap();
    let r = &design.references[0];
    assert_eq!((r.number, r.model, r.file_name.as_str()), (1, Some(0), "PW_WORKDIR:d0105170\\plan.dgn"));
    assert_eq!((r.master_origin, r.matrix[4], r.scale), ([5000.0, 0.0, 0.0], 1.0, 1.0));
    let s = scene(&design);
    assert_eq!(info(&s, "DGN reference files"), "PW_WORKDIR:d0105170\\plan.dgn; AUROW.DGN");
}

/// A 32-bit integer low word first, as in V7 shared cells (§A.8).
fn long_le(v: i32) -> [u16; 2] { [v as u32 as u16, ((v as u32) >> 16) as u16] }

/// The words 18 to 89 of a V7 shared cell definition or instance (§A.8): the row-major matrix
/// at word 38, the origin at word 74, the name at word 82.
fn v7_shared(matrix: [f64; 9], origin: [i32; 3], name: &str) -> Vec<u16> {
    let mut body = vec![0u16; 90 - 18];
    for (k, value) in matrix.iter().enumerate() { body[38 - 18 + 4 * k..42 - 18 + 4 * k].copy_from_slice(&dfloat(*value)); }
    for (k, value) in origin.iter().enumerate() { body[74 - 18 + 2 * k..76 - 18 + 2 * k].copy_from_slice(&long_le(*value)); }
    for (k, pair) in name.as_bytes().chunks(2).enumerate() { body[82 - 18 + k] = u16::from(pair[0]) | u16::from(*pair.get(1).unwrap_or(&0)) << 8; }
    body
}

#[test]
fn v7_shared_cells_place_their_definition() {
    const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    let mut w = Writer::new(true, "M", "MM", 1000, 1, [0.0; 3]);
    // The definition POST: a line of 1 m along X, in definition coordinates.
    let line = w.points(&[[0, 0, 0], [1000, 0, 0]]);
    let mut definition = v7_shared(IDENTITY, [0; 3], "POST");
    definition[0] = ((18 + definition.len()) + (18 + line.len()) - 19) as u16;
    w.element(34, 3, 0, 0, 0, &definition);
    w.element(3, 3, 2, 0, 0x80, &line);
    // An instance turned +90 degrees about Z (row by row), at (5, 0, 0) m, and one of a name
    // without a definition.
    w.element(35, 4, 0, 0, 0, &v7_shared([0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [5000, 0, 0], "POST"));
    w.element(35, 4, 0, 0, 0, &v7_shared(IDENTITY, [0; 3], "GONE"));
    let design = dgn_read::read(&w.bytes()).unwrap();
    assert_eq!(design.shared_cells.len(), 1);
    assert_eq!(design.shared_cells[0].name, "POST");
    assert_eq!(design.models[0].elements.len(), 2, "the definition is not drawn where it stands");
    let s = scene(&design);
    assert_eq!(info(&s, "DGN elements"), "Shared cell: 1");
    assert_eq!(info(&s, "DGN skipped elements"), "Shared cell without definition: 1");
    // The component keeps its own level (3); the line runs along +Y from the instance origin.
    assert_eq!(s.layers[0].name, "Level 3");
    let (lo, hi) = bounds(&s.layers[0]);
    assert!(lo[1] > -0.02 && (hi[1] - 1.0).abs() < 0.02 && hi[0] - lo[0] < 0.02, "{lo:?} {hi:?}");
    assert_eq!(info(&s, "DGN world origin (metres)"), "5, 0, 0");
    let _ = long(0);
}
