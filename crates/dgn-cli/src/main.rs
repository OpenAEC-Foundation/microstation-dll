use std::fmt::Write as _;
use std::process::ExitCode;

use dgn_model::{kind_name, Format};
use serde_json::Value;

const USAGE: &str = "usage:
  dgn info <file.dgn>                      format, revision, units, levels, models, element counts
  dgn json <file.dgn>                      the whole neutral model as JSON
  dgn drawing <file.dgn> [--model n]       line work and faces in world metres, as JSON
  dgn scene <file.dgn> [--model n]         what the viewer scene holds: layers, triangles, info
  dgn obj <file.dgn> [--model n] -o out.obj
                                           the viewer scene as Wavefront OBJ (one group per level)
  dgn dump <file.dgn>                      the raw elements: offset or stream, type, level, size
  dgn tree <file.dgn>                      the storages and streams of a V8 compound file
JSON commands print to standard output; add `-o out.json` to write a file instead, `--pretty` to indent.";

/// The value after a `--name` option.
fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn model(args: &[String]) -> Result<usize, String> {
    opt(args, "--model").map_or(Ok(0), |m| m.parse().map_err(|_| format!("--model needs a number, not {m}")))
}

fn read(path: &str) -> Result<dgn_model::Design, String> { dgn_read::read_file(path).map_err(String::from) }

/// What the viewer scene holds.
fn scene_summary(scene: &dgn_export::Scene) -> Value {
    serde_json::json!({
        "world_origin": scene.world_origin,
        "info": scene.info.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
        "layers": scene.layers.iter().map(|l| serde_json::json!({
            "level": l.level, "name": l.name, "triangles": l.indices.len() / 3, "elements": l.elements.len(),
        })).collect::<Vec<_>>(),
    })
}

/// The scene as Wavefront OBJ: vertices in metres (Z up, from the scene origin), one group per
/// level, vertex colours as the common `v x y z r g b` extension.
fn obj(scene: &dgn_export::Scene) -> String {
    let mut out = String::new();
    let o = scene.world_origin;
    let _ = writeln!(out, "# MicroStation DGN scene, metres from world point {} {} {}", o[0], o[1], o[2]);
    let mut base = 1;
    for layer in &scene.layers {
        let _ = writeln!(out, "g {}", layer.name.replace(char::is_whitespace, "_"));
        for (p, c) in layer.positions.chunks_exact(3).zip(&layer.colors) {
            let _ = writeln!(out, "v {} {} {} {:.4} {:.4} {:.4}", p[0], p[1], p[2], f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0);
        }
        for n in layer.normals.chunks_exact(3) { let _ = writeln!(out, "vn {} {} {}", n[0], n[1], n[2]); }
        for t in layer.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| i as usize + base);
            let _ = writeln!(out, "f {a}//{a} {b}//{b} {c}//{c}");
        }
        base += layer.positions.len() / 3;
    }
    out
}

/// The raw elements of a file, one line each.
fn dump(path: &str) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let mut out = String::new();
    if !data.starts_with(&dgn_read::OLE_SIGNATURE) {
        let _ = writeln!(out, "V7 design file, {} bytes", data.len());
        for e in dgn_read::v7::elements(&data).map_err(String::from)? {
            let _ = writeln!(out, "{:>10}  type {:>3} {:<24} level {:>2}{}  {} words",
                e.offset, e.kind, kind_name(e.kind.into()), e.level, if e.complex { " component" } else { "" }, e.words());
        }
        return Ok(out);
    }
    let streams = dgn_read::v8::streams(&data).map_err(String::from)?;
    let _ = writeln!(out, "V8 design file, {} bytes, revision {:?}, application {:?}", data.len(), streams.revision, streams.application);
    let section = |title: String, stream: &[u8], out: &mut String| -> Result<(), String> {
        let (elements, broken) = dgn_read::v8::split(stream).map_err(String::from)?;
        let _ = writeln!(out, "{title}: {} elements in {} bytes{}", elements.len(), stream.len(), if broken { " (rest not readable)" } else { "" });
        for e in elements {
            let _ = writeln!(out, "  type {:>3} {:<24} flags {:04X} level {:>6} id {:>8}  {} bytes{}",
                e.kind(), kind_name(e.kind()), e.flags(), e.level(), e.id(), e.len(),
                e.name().map_or(String::new(), |n| format!("  \"{n}\"")));
        }
        Ok(())
    };
    section("Dgn^Nm".into(), &streams.tables, &mut out)?;
    for m in &streams.models {
        let storage = if m.hidden { "Dgn-Nd" } else { "Dgn-Md" };
        section(format!("{storage}/#{:06x}/Dgn^G", m.id), &m.graphics, &mut out)?;
        section(format!("{storage}/#{:06x}/Dgn^C", m.id), &m.controls, &mut out)?;
    }
    for p in &streams.problems { let _ = writeln!(out, "problem: {p}"); }
    Ok(out)
}

/// The compound file tree of a V8 file.
fn tree(path: &str) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let file = dgn_read::cfb::CompoundFile::open(&data).map_err(String::from)?;
    let mut out = String::new();
    for (name, k) in file.tree() {
        let e = &file.entries[k];
        if e.kind == dgn_read::cfb::STORAGE { let _ = writeln!(out, "{name}/"); } else { let _ = writeln!(out, "{name}  {} bytes", e.size); }
    }
    Ok(out)
}

fn run(args: &[String]) -> Result<(), String> {
    let (Some(command), Some(path)) = (args.first(), args.get(1)) else { return Err(USAGE.into()) };
    let output = opt(args, "-o");
    let emit = |text: String| -> Result<(), String> {
        match output {
            Some(file) => std::fs::write(file, text).map_err(|e| format!("cannot write {file}: {e}")),
            None => { println!("{text}"); Ok(()) }
        }
    };
    let json = |value: Value| -> Result<(), String> {
        emit(if args.iter().any(|a| a == "--pretty") { serde_json::to_string_pretty(&value).unwrap_or_default() } else { value.to_string() })
    };
    match command.as_str() {
        "info" => {
            let design = read(path)?;
            let mut summary = dgn_export::json::summary(&design);
            if design.format == Format::V8 { summary["revision_label"] = design.revision_label().into(); }
            json(summary)
        }
        "json" => json(dgn_export::json::design(&read(path)?)),
        "drawing" => json(dgn_export::json::drawing(&dgn_export::draw_model(&read(path)?, model(args)?))),
        "scene" => json(scene_summary(&dgn_export::draw_model(&read(path)?, model(args)?).scene()?)),
        "obj" => {
            if output.is_none() { return Err("obj needs -o out.obj".into()); }
            emit(obj(&dgn_export::draw_model(&read(path)?, model(args)?).scene()?))
        }
        "dump" => emit(dump(path)?),
        "tree" => emit(tree(path)?),
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => { eprintln!("{e}"); ExitCode::FAILURE }
    }
}
