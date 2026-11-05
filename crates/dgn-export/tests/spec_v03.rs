//! Coverage added with `SPEC_v0.3.md`: subtree counts of complex headers, signed V7 text
//! multipliers, sheet models, attachments of models of the same file, raster. Files are written
//! in the test.

use dgn_export::{Scene, SceneLayer};
use dgn_read::synth::v7::{dfloat, long, Writer};
use dgn_read::synth::v8::*;

fn scene(bytes: Vec<u8>) -> Scene {
    dgn_export::scene(&dgn_read::read(&bytes).unwrap()).unwrap()
}

fn info<'a>(scene: &'a Scene, key: &str) -> &'a str {
    scene.info.iter().find(|(k, _)| k == key).map_or_else(|| panic!("no {key} in {:?}", scene.info), |(_, v)| v.as_str())
}

fn bounds(layer: &SceneLayer) -> ([f32; 3], [f32; 3]) {
    layer.positions.chunks_exact(3).fold(([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]), |(mut lo, mut hi), p| {
        for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); }
        (lo, hi)
    })
}

#[test]
fn v8_complex_headers_own_their_whole_subtree() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    // A cell (count 5) of a complex shape (count 2) and a text node (count 1) with their
    // components; then a top-level line that is not part of the cell.
    f.graphic(element(2, 0x3000, 1, shown(0, 0), &[5, 0, 0, 0, 0, 0, 0, 0], &[]));
    f.graphic(element(14, 0x7000, 1, shown(0, 0), &2u32.to_le_bytes(), &[]));
    f.graphic(element(4, 0x5000, 1, shown(0, 0), &counted(2, &[0.0, 0.0, 1000.0, 0.0]), &[]));
    f.graphic(element(4, 0x5000, 1, shown(0, 0), &counted(3, &[1000.0, 0.0, 1000.0, 1000.0, 0.0, 0.0]), &[]));
    f.graphic(element(7, 0x7000, 1, shown(0, 0), &[1, 0, 0, 0, 1, 0, 0, 0], &[]));
    f.graphic(element(17, 0x5000, 1, shown(0, 0), &text_data([300.0, 300.0], [0.0, 2000.0], b"A"), &[]));
    f.graphic(element(3, 0x1000, 2, shown(0, 0), &doubles(&[5000.0, 0.0, 6000.0, 0.0]), &[]));
    let s = scene(f.bytes());
    assert_eq!(info(&s, "DGN elements"), "Cell: 1, Line: 1");
    assert_eq!(info(&s, "DGN skipped elements"), "0", "no nesting or complex shape component problems");
    assert!(s.layers[0].props[0].1.contains("\"Text\":\"A\""), "{}", s.layers[0].props[0].1);
}

#[test]
fn v7_negative_text_multipliers_mirror_the_text() {
    // 1 UOR = 1 mm. A character "-" with both multipliers -2 400 000 (0xFFDB6100): 14.4 m,
    // running from its origin towards -x and -y. Read unsigned it would be 25.8 km.
    let mut w = Writer::new(false, "M", "MM", 1000, 1, [0.0; 3]);
    let mut text = vec![0x0000];
    text.extend(long(-2_400_000));
    text.extend(long(-2_400_000));
    text.extend(long(0));
    text.extend(w.points(&[[100_000, 100_000, 0]]));
    text.extend([1, u16::from(b'-')]);
    w.element(17, 1, 0, 0, 0, &text);
    let s = scene(w.bytes());
    // The world origin is the text origin; the box spans -14.4..0 both ways.
    let (lo, hi) = bounds(&s.layers[0]);
    assert!((lo[0] + 14.4).abs() < 0.1 && hi[0].abs() < 0.1 && (lo[1] + 14.4).abs() < 0.1 && hi[1].abs() < 0.1, "{lo:?} {hi:?}");
}

#[test]
fn v8_sheet_models_and_attachments_of_models_of_the_same_file() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    f.header = model_header_stream(1000.0, 1.0, 1.0, [0.0; 3], Some("Default"), false);
    f.graphic(element(3, 0x1000, 1, shown(0, 0), &doubles(&[0.0, 0.0, 1000.0, 0.0]), &[]));
    // The default model attaches the sheet "Border" 10 m to the right (scale stored as 0: 1).
    f.control(model_attachment(1, "Border", "Bord_1", [10_000.0, 0.0, 0.0], 0.0));
    // The border: a line of 2 m on level 2; it attaches the default model back (a cycle).
    let mut border = Vec::new();
    push(&mut border, element(3, 0x1000, 2, shown(0, 0), &doubles(&[0.0, 0.0, 2000.0, 0.0]), &[]));
    let mut controls = Vec::new();
    push(&mut controls, model_attachment(1, "Default", "Back", [0.0; 3], 1.0));
    f.models.push(ExtraModel {
        id: 1, hidden: false, header: model_header_stream_with(1000.0, 1.0, 1.0, [0.0; 3], Some("Border"), false, true), graphics: border, controls,
    });
    let design = dgn_read::read(&f.bytes()).unwrap();
    assert!(design.models[1].sheet && !design.models[0].sheet);
    let r = &design.references[0];
    assert_eq!((r.file_name.as_str(), r.model_name.as_deref(), r.logical_name.as_str(), r.scale), ("", Some("Border"), "Bord_1", 1.0));
    let s = dgn_export::scene(&design).unwrap();
    assert_eq!(info(&s, "DGN models"), "2: Default, Border (sheet)");
    assert_eq!(info(&s, "DGN elements"), "Line: 2, Reference model: 1");
    assert_eq!(info(&s, "DGN skipped elements"), "Reference that attaches itself: 1");
    assert_eq!(info(&s, "DGN reference files"), "model Border (Bord_1)");
    let border = s.layers.iter().find(|l| l.name == "Level 2").unwrap();
    let (lo, hi) = bounds(border);
    assert!((lo[0] - 10.0).abs() < 0.05 && (hi[0] - 12.0).abs() < 0.05, "{lo:?} {hi:?}");
    assert!(border.props[0].1.contains("\"Reference\":\"model Border (Bord_1)\""), "{}", border.props[0].1);
}

#[test]
fn v8_raster_pixels_are_faces_in_their_colours() {
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    // 3 x 2 pixels of 1 m from (0, 2) m down: blue blue -, green - - (0 is transparent).
    for e in raster(1, &[&[1, 1, 0], &[2, 0, 0]], 1000.0, [0.0, 2000.0, 0.0]) { f.graphic(e); }
    let s = scene(f.bytes());
    assert_eq!(info(&s, "DGN elements"), "Raster: 1");
    let layer = &s.layers[0];
    assert_eq!(layer.indices.len() / 3, 4, "a blue run and a green pixel");
    assert_eq!(layer.colors.iter().filter(|c| **c == [0, 0, 255, 255]).count(), 6);
    assert_eq!(layer.colors.iter().filter(|c| **c == [0, 255, 0, 255]).count(), 6);
    let (lo, hi) = bounds(layer);
    assert!(((hi[0] - lo[0]) - 2.0).abs() < 1e-5 && ((hi[1] - lo[1]) - 2.0).abs() < 1e-5, "{lo:?} {hi:?}");
}

#[test]
fn v7_raster_in_run_lengths() {
    // 1 UOR = 1 mm; a raster of 4 x 1 pixels of 1 m at (0, 1) m, format 9, foreground 3 (red),
    // background 0 transparent: runs 1 (background) and 3 (foreground).
    let mut w = Writer::new(false, "M", "MM", 1000, 1, [0.0; 3]);
    let flags: u16 = (9 << 3) | 0x4 | 0x200;
    let mut header = vec![0u16, 0, flags, 3, 4, 1, 0, 0];
    header.extend([0u16; 4]);
    header.extend(dfloat(1000.0));
    for v in [0, 1000, 0] { header.extend(long(v)); }
    header.push(0);
    w.element(87, 1, 0, 0, 0, &header);
    w.element(88, 1, 0, 0, 0x80, &[flags, 3, 0, 0, 4, 1, 3]);
    let s = scene(w.bytes());
    assert_eq!(info(&s, "DGN elements"), "Raster: 1");
    let layer = &s.layers[0];
    assert_eq!(layer.indices.len() / 3, 2);
    assert!(layer.colors.iter().all(|c| *c == [255, 0, 0, 255]));
    let (lo, hi) = bounds(layer);
    assert!(((hi[0] - lo[0]) - 3.0).abs() < 1e-5 && ((hi[1] - lo[1]) - 1.0).abs() < 1e-5, "{lo:?} {hi:?}");
}
