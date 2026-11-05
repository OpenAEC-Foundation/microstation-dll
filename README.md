# microstation-dll

Native library (DLL) that reads MicroStation design files (`.dgn`) of all versions without
MicroStation, for [Open 3D Viewer Studio](https://github.com/OpenAEC-Foundation/open-3d-viewer),
[OpenCAD Studio](https://github.com/OpenAEC-Foundation/OpenCADStudio) and other tools.

**Status:** private, pre-alpha. Do not make this repository public without a decision by the
maintainers (see "Legal" below).

## Scope

- Input: MicroStation design files, V7 (IGDS / ISFF) and V8 (V8 2004 Edition, V8i, CONNECT
  Edition, 2023 and later), 2D and 3D. V8 cell libraries (`.cel`) open as design files with one
  model per cell.
- Output: a neutral model (`dgn-model`): models, levels with symbology, colour table, units and
  global origin, elements with their display header (level, colour, weight, style, class,
  fill) and geometry, cells, shared cells with their definitions, text, dimensions, curves,
  B-splines, surfaces, solids, meshes and reference attachments. From that model `dgn-export`
  makes line work and faces in world metres, a triangle scene for viewers, and JSON.
- No writing of DGN files.

| Crate | What it is |
|---|---|
| `dgn-model` | The neutral model; serde (JSON) round trip. No dependencies beyond serde. |
| `dgn-read` | Compound file ([MS-CFB]), zlib, V7 and V8 element parsing into the model. Feature `synth`: writers of synthetic DGN files for tests. |
| `dgn-export` | `draw` / `draw_model`: polylines and faces in metres per element; `Drawing::scene`: one mesh per level (lines as ribbons) for viewers; `json`. |
| `dgn-ffi` | The C ABI: `dgn.dll` / `libdgn.so` / `libdgn.dylib`, header [`include/dgn.h`](include/dgn.h). |
| `dgn-cli` | The `dgn` tool: `info`, `json`, `drawing`, `scene`, `obj`, `dump`, `tree`. |

`dgn-model`, `dgn-read` and `dgn-export` have no GUI or GPU dependencies and build for
`wasm32-unknown-unknown`.

## Version support

From the clean-room specification, `SPEC_v0.2.md` §C (sample files measured by the analysis
team; this repository contains none). ✓ read and tested, ~ partly, – no sample of that version
yet (read the same way, untested), ✗ not specified yet: skipped and counted.

| | V7 2D | V7 3D | V8 2004 (rev 5) | V8i (rev 103/104) | CONNECT and 2023+ (rev 202) |
|---|---|---|---|---|---|
| Recognition, container, element framing | ✓ | ✓ | ✓ | ✓ | ✓ |
| Units, global origin | ✓ | ✓ | ✓ | ✓ | ✓ |
| Levels (V7: numbers), colour table, by-level symbology | ✓ | ✓ | ✓ | ✓ | ✓ |
| Line, line string, shape, arc, ellipse, curve, complex chain / shape | ✓ | ✓ | ✓ | ✓ | ✓ |
| Text, text node | ✓ | ✓ | ✓ | ✓ | ✓ |
| Cell | ✓ | ✓ | ✓ | ✓ | ✓ |
| Shared cell (definition 34, instance 35) | – | ✓ | ✓ | ✓ | ✓ |
| Fill (filled shapes and cells) | n/a | n/a | ✓ | ✓ | ✓ |
| B-spline curve | ✓ | – | ✓ | ✓ | ✓ |
| B-spline surface | ~ (ISFF) | ~ (ISFF) | – | – | ✓ |
| Surface / solid of projection (18/19) | n/a | ✓ | – | – | ✓ |
| Surface / solid of revolution | n/a | ~ outlines | – | – | ~ outlines |
| Cone / cylinder | n/a | ✓ (ISFF) | – | – | ✓ |
| Mesh (105) | n/a | n/a | – | – | ✓ |
| Smart solid / surface (Parasolid B-rep) | n/a | n/a | – | ~ wireframe | ~ wireframe |
| Dimension (33) | ✗ | ✗ | ~ 2D linear | ~ 2D linear | – |
| Multi-line (36), tag (37), raster | ✗ | ✗ | ✗ | ✗ | ✗ |
| Several models (design models, sheets, `.cel` cells, hidden models) | n/a | n/a | ✓ | ✓ | ✓ |
| Reference attachments (as data; the referenced file is not loaded) | ✓ (ISFF) | ✓ (ISFF) | – | ~ | – |
| Format revision, writing application | n/a | n/a | ✓ | ✓ | ✓ |

MicroStation 2023 to 2025 files have not been seen without OpenRoads; OpenRoads Designer 2024
writes revision 202 like CONNECT. An unknown, higher revision is read the same way and reported.
Extended colours (256 and up) draw white; custom line styles draw solid.

## Use

Rust (pin a revision, like the viewer does):

```toml
dgn-read = { git = "https://github.com/OpenAEC-Foundation/microstation-dll", rev = "<commit>" }
dgn-model = { git = "https://github.com/OpenAEC-Foundation/microstation-dll", rev = "<commit>" }
dgn-export = { git = "https://github.com/OpenAEC-Foundation/microstation-dll", rev = "<commit>" }
```

```rust
let design = dgn_read::read(&bytes)?;                  // or read_file(path)
for e in &design.models[0].elements {
    let symbology = design.resolve(e.symbology, e.level); // "by level" values resolved
    let rgb = design.rgb(symbology.color);
    // e.geometry: Line, LineString, Ellipse, Text, Cell, SharedCell, BSplineCurve, Mesh, ...
}
let drawing = dgn_export::draw(&design);               // polylines and faces in metres
let scene = drawing.scene()?;                          // one mesh per level, for viewers
```

C (`cargo build --release -p dgn-ffi` gives `target/release/dgn.dll`, `dgn.dll.lib` and
`libdgn.a`/`dgn.lib`; see [`include/dgn.h`](include/dgn.h)):

```c
DgnDocument *doc = dgn_open("drawing.dgn");
char *info = dgn_query(doc, "{\"query\": \"info\"}");   /* JSON */
dgn_free(info);
DgnScene *scene = dgn_scene(doc, 0);                    /* triangle meshes per level */
dgn_scene_free(scene);
dgn_close(doc);
```

Command line (`cargo build --release -p dgn-cli` gives `dgn`):

```
dgn info drawing.dgn --pretty
dgn obj drawing.dgn -o drawing.obj
dgn dump drawing.dgn
```

Tests: `cargo test`. Every test builds its DGN bytes in the test from the specification
(`dgn_read::synth`); there are no sample files in the repository.

## Open 3D Viewer Studio

The viewer's `src/loaders/dgn.rs` is a thin adapter: `dgn_read::read` → `dgn_export::draw` →
`Drawing::scene`, whose layers map one to one onto the viewer's `Layer` (positions, normals,
colours, indices, pickable elements, properties) and whose `info` rows go to the project
information. The dependency is a git dependency pinned to a commit of this repository.

## OpenCAD Studio

OpenCAD Studio reads DWG and DXF with `codec` (the `opencaddocument` crate): `io::load_bytes`
picks `DwgReader` or `DxfReader` by extension into a `codec::Document`, and its other imports
(`io::model_import`: SAT, STEP, IGES) return `Imported { blocks, entities }` of
`codec::EntityType`. A DGN import fits the same shape, through `dgn-read` and `dgn-model`
(`dgn-export` where it saves work):

1. `load_bytes`: a `"dgn"` arm (and `"cel"`) calling `dgn_read::read(&bytes)`; on the web the
   same call works in the worker, since `dgn-read` builds for wasm.
2. Units: drawing units from `design.units` (`metres_per_master`, `master_name`);
   coordinates through `design.to_master(p)` (master units from the global origin) or
   `to_metres`.
3. Levels → layers: `design.levels` (V7: numbers 1–63, name `design.level_name(n)`), colour
   from the level's symbology. Element colours with `design.resolve(e.symbology, e.level)` and
   `design.rgb(...)`, as true colour (`Color::Rgb`): DGN colour indices are not ACI indices.
   Weight 0–31 → lineweight; styles 0–7 → standard linetypes.
4. Geometry, element by element:
   - `Line`, `LineString` → line / polyline (closed for shapes; `fill` → a solid hatch);
   - `Ellipse` → circle, arc or ellipse (centre, major axis `axis_a`, ratio `|axis_b| / |axis_a|`,
     parameters `start .. start + sweep`);
   - `BSplineCurve` → spline: degree `order − 1`, control points `poles`, `weights`, and the full
     `knots` (for `periodic`, append the first `order − 1` poles);
   - `Curve` (type 11), `Dimension`, `Cone`, `Surface`, `BSplineSurface` → the polylines and faces
     of `dgn_export::draw` for that element (or native entities where OpenCAD has them; a
     `Dimension` carries its points, feet and measured distances);
   - `Text` → text at `origin` with the height of the box and the rotation of `x_axis`;
     `TextNode` → one text per line or an MText;
   - `Cell` → a group, or an anonymous block inserted at the origin (its components are already
     in design coordinates); `SharedCell` → an `Insert` of a block made once per
     `design.shared_cells[definition]` (block entities in definition coordinates; the instance's
     `matrix` and `origin` give scale, rotation and insertion point; skewed matrices → explode);
   - `Mesh` → polyface mesh.
5. Models: `design.models[0]` to model space; sheets and other models to layouts or blocks.
6. `design.references` → external references when the file is found beside the drawing.

OpenCAD Studio already has a `src/app/commands/dgn_import.rs`. Under the clean-room rules of
this repository its content was not opened; whether to replace it with `dgn-read` is for the
OpenCAD Studio maintainers to decide after checking where it came from.

## Clean-room rules

The DGN format work runs with strict role separation (protocol in the analysis workspace,
following Directive 2009/24/EC art. 5(3) and 6 and CJEU C-406/10 SAS/WPL):

- The **analysis team** studies public sample files as black boxes (hex dumps, statistics,
  comparison with plots and exports) and writes the specification (`SPEC_v*.md`).
- The **implementation team** (this repository) writes code only from that specification, from
  Intergraph's public "Intergraph Standard File Formats (Element Structure)" (ISFF, V7) and from
  Microsoft's public [MS-CFB] and [MS-OLEPS]. Questions go to the analysis team in writing.
- Never: disassembling, decompiling or debugging Bentley, ODA or other DGN software; reading the
  source or internal documentation of other DGN readers (GDAL/OGR DGN and DGNv8, dgnlib,
  ODA/Teigha, v8file.net, libdgnv8 and others); Bentley's licensed "V8 DGN File Format
  Reference" or MicroStation SDK/MDL headers; copying Bentley content (colour tables, line
  styles, fonts, cells).
- Contributors who have seen such material do not write reader code.

## Test data

Never commit `.dgn`, `.cel`, `.dgnlib` or other MicroStation files, sample files of third
parties, or exports derived from them; `.gitignore` enforces the file types. Tests build their
files from the specification in the test.

## Legal

MicroStation is a registered trademark of Bentley Systems, Incorporated. This project is not
affiliated with, endorsed or sponsored by Bentley Systems. Intergraph is a trademark of
Hexagon AB. Before any public release: legal review of the clean-room protocol and a patent
freedom-to-operate check.
