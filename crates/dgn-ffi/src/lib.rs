//! C ABI of the DGN reader (`dgn.dll`, `libdgn.so`, `libdgn.dylib`); the header is
//! `include/dgn.h`.
//!
//! ```c
//! DgnDocument *doc = dgn_open("drawing.dgn");          // NULL on failure: see dgn_last_error()
//! char *json = dgn_query(doc, "{\"query\": \"info\"}");
//! /* ... */
//! dgn_free(json);
//! DgnScene *scene = dgn_scene(doc, 0);                  // triangle meshes of model 0
//! for (size_t i = 0; i < dgn_scene_layer_count(scene); i++) {
//!     DgnLayer layer;
//!     dgn_scene_layer(scene, i, &layer);
//!     /* layer.positions, layer.normals, layer.colors, layer.indices ... */
//! }
//! dgn_scene_free(scene);
//! dgn_close(doc);
//! ```
//!
//! `dgn_query` takes a JSON request and returns a JSON answer, `{"ok": true, "result": …}` or
//! `{"ok": false, "error": "…"}`, which the caller frees with `dgn_free`. Requests:
//! - `{"query": "info"}`: format, revision, units, origin, levels, models, element counts,
//!   references, skipped elements;
//! - `{"query": "models"}`: index, ID, name, 2D/3D, hidden and number of elements per model;
//! - `{"query": "levels"}`: ID, name, description and symbology per level;
//! - `{"query": "elements", "model": 0, "offset": 0, "limit": 100}`: elements of a model with
//!   their geometry in design coordinates (UOR), `{"total", "elements"}`;
//! - `{"query": "shared_cells"}`: shared cell definitions with their components;
//! - `{"query": "references"}`: reference attachments;
//! - `{"query": "design"}`: the whole model;
//! - `{"query": "drawing", "model": 0}`: line work and faces in world metres with level,
//!   colour and weight, and the properties of each element.

use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};

use dgn_export::Scene;
use dgn_model::Design;
use serde_json::{json, Value};

/// An open document behind the C ABI.
pub struct DgnDocument(Design);

/// The triangle scene of one model.
pub struct DgnScene {
    scene: Scene,
    names: Vec<CString>,
}

/// One layer of a scene, pointing into the scene (valid until `dgn_scene_free`).
#[repr(C)]
pub struct DgnLayer {
    pub level: u32,
    /// UTF-8, NUL-terminated.
    pub name: *const c_char,
    /// `vertex_count` × xyz in metres from the scene origin.
    pub positions: *const f32,
    /// `vertex_count` × xyz, unit length.
    pub normals: *const f32,
    /// `vertex_count` × RGBA, sRGB.
    pub colors: *const u8,
    pub vertex_count: usize,
    /// Triangle list, `index_count` indices.
    pub indices: *const u32,
    pub index_count: usize,
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(message: &str) {
    let text = CString::new(message.replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = text);
}

/// Answers one JSON request on a design (the safe core of `dgn_query`).
pub fn query(design: &Design, request: &str) -> Value {
    match answer(design, request) {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

fn answer(design: &Design, request: &str) -> Result<Value, String> {
    let req: Value = serde_json::from_str(request).map_err(|e| format!("request is not JSON: {e}"))?;
    let q = req.get("query").and_then(Value::as_str).ok_or("request has no \"query\"")?;
    let to = |v: Result<Value, serde_json::Error>| v.map_err(|e| e.to_string());
    let model = || -> Result<usize, String> {
        let index = req.get("model").and_then(Value::as_u64).unwrap_or(0) as usize;
        if index < design.models.len() { Ok(index) } else { Err(format!("no model {index} (the file has {})", design.models.len())) }
    };
    match q {
        "info" => Ok(dgn_export::json::summary(design)),
        "models" => Ok(design.models.iter().enumerate().map(|(index, m)| json!({
            "index": index, "id": m.id, "name": m.name, "description": m.description, "three_d": m.three_d,
            "hidden": m.hidden, "elements": m.elements.len(), "units": m.units,
        })).collect()),
        "levels" => to(serde_json::to_value(&design.levels)),
        "elements" => {
            let m = &design.models[model()?];
            let limit = req.get("limit").and_then(Value::as_u64).map_or(usize::MAX, |n| n as usize);
            let offset = req.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let elements: Vec<_> = m.elements.iter().skip(offset).take(limit).collect();
            Ok(json!({"model": m.id, "total": m.elements.len(), "elements": to(serde_json::to_value(elements))?}))
        }
        "shared_cells" => to(serde_json::to_value(&design.shared_cells)),
        "references" => to(serde_json::to_value(&design.references)),
        "design" => Ok(dgn_export::json::design(design)),
        "drawing" => Ok(dgn_export::json::drawing(&dgn_export::draw_model(design, model()?))),
        other => Err(format!("unknown query {other:?} (info, models, levels, elements, shared_cells, references, design, drawing)")),
    }
}

fn c_string(v: &Value) -> *mut c_char {
    // serde_json escapes control characters, so the text has no interior NUL
    CString::new(v.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

fn open_with(read: impl FnOnce() -> dgn_read::Result<Design> + std::panic::UnwindSafe) -> *mut DgnDocument {
    match std::panic::catch_unwind(read) {
        Ok(Ok(design)) => Box::into_raw(Box::new(DgnDocument(design))),
        Ok(Err(e)) => { set_error(&e.0); std::ptr::null_mut() }
        Err(_) => { set_error("internal error while reading the file"); std::ptr::null_mut() }
    }
}

/// The library version, such as `0.1.0` (static, not freed).
#[no_mangle]
pub extern "C" fn dgn_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Why the last `dgn_open`, `dgn_open_bytes` or `dgn_scene` of this thread failed; empty when it
/// did not. Valid until the next call of those functions on this thread; not freed.
#[no_mangle]
pub extern "C" fn dgn_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// Opens a `.dgn` file (UTF-8 path). Returns NULL when the path is NULL or the file cannot be read.
///
/// # Safety
/// `path` must be NULL or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn dgn_open(path: *const c_char) -> *mut DgnDocument {
    set_error("");
    if path.is_null() { set_error("null path"); return std::ptr::null_mut(); }
    let Ok(p) = CStr::from_ptr(path).to_str() else { set_error("path is not UTF-8"); return std::ptr::null_mut() };
    let p = p.to_string();
    open_with(move || dgn_read::read_file(p))
}

/// Reads a design file from memory (`len` bytes at `data`, copied). Returns NULL when it cannot
/// be read.
///
/// # Safety
/// `data` must point to `len` readable bytes (or be NULL with `len` 0).
#[no_mangle]
pub unsafe extern "C" fn dgn_open_bytes(data: *const u8, len: usize) -> *mut DgnDocument {
    set_error("");
    let bytes = if len == 0 { &[][..] } else if data.is_null() { set_error("null data"); return std::ptr::null_mut() } else { std::slice::from_raw_parts(data, len) };
    open_with(move || dgn_read::read(bytes))
}

/// Answers a JSON request (see the crate documentation). The result is freed with `dgn_free`;
/// NULL only when the answer could not be allocated.
///
/// # Safety
/// `doc` must be NULL or a pointer from `dgn_open` not yet closed; `request` NULL or a
/// NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn dgn_query(doc: *const DgnDocument, request: *const c_char) -> *mut c_char {
    if doc.is_null() || request.is_null() {
        return c_string(&json!({"ok": false, "error": "null document or request"}));
    }
    let Ok(r) = CStr::from_ptr(request).to_str() else { return c_string(&json!({"ok": false, "error": "request is not UTF-8"})) };
    let d = &(*doc).0;
    let v = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| query(d, r))).unwrap_or_else(|_| json!({"ok": false, "error": "internal error"}));
    c_string(&v)
}

/// Frees a string returned by `dgn_query`.
///
/// # Safety
/// `s` must be NULL or a pointer from `dgn_query` not yet freed.
#[no_mangle]
pub unsafe extern "C" fn dgn_free(s: *mut c_char) {
    if !s.is_null() { drop(CString::from_raw(s)); }
}

/// Closes a document from `dgn_open`.
///
/// # Safety
/// `doc` must be NULL or a pointer from `dgn_open` not yet closed.
#[no_mangle]
pub unsafe extern "C" fn dgn_close(doc: *mut DgnDocument) {
    if !doc.is_null() { drop(Box::from_raw(doc)); }
}

/// Builds the triangle scene of model `model` (0: the default model): one mesh per level, lines
/// as thin ribbons. NULL when it has nothing to draw (see `dgn_last_error`). The scene stays
/// valid after the document is closed; free it with `dgn_scene_free`.
///
/// # Safety
/// `doc` must be NULL or a pointer from `dgn_open` not yet closed.
#[no_mangle]
pub unsafe extern "C" fn dgn_scene(doc: *const DgnDocument, model: usize) -> *mut DgnScene {
    set_error("");
    if doc.is_null() { set_error("null document"); return std::ptr::null_mut(); }
    let design = &(*doc).0;
    if model >= design.models.len() { set_error("no such model"); return std::ptr::null_mut(); }
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dgn_export::draw_model(design, model).scene()));
    match built {
        Ok(Ok(scene)) => {
            let names = scene.layers.iter().map(|l| CString::new(l.name.replace('\0', " ")).unwrap_or_default()).collect();
            Box::into_raw(Box::new(DgnScene { scene, names }))
        }
        Ok(Err(e)) => { set_error(&e); std::ptr::null_mut() }
        Err(_) => { set_error("internal error while drawing"); std::ptr::null_mut() }
    }
}

/// The number of layers of a scene (0 for NULL).
///
/// # Safety
/// `scene` must be NULL or a pointer from `dgn_scene` not yet freed.
#[no_mangle]
pub unsafe extern "C" fn dgn_scene_layer_count(scene: *const DgnScene) -> usize {
    if scene.is_null() { 0 } else { (*scene).scene.layers.len() }
}

/// The scene's origin in world metres (the first point drawn): three doubles at `xyz`.
///
/// # Safety
/// `scene` must be NULL or a pointer from `dgn_scene` not yet freed; `xyz` NULL or room for 3 doubles.
#[no_mangle]
pub unsafe extern "C" fn dgn_scene_origin(scene: *const DgnScene, xyz: *mut f64) {
    if scene.is_null() || xyz.is_null() { return; }
    std::ptr::copy_nonoverlapping((*scene).scene.world_origin.as_ptr(), xyz, 3);
}

/// Fills `out` with layer `index`; returns 0 when there is no such layer.
///
/// # Safety
/// `scene` must be NULL or a pointer from `dgn_scene` not yet freed; `out` NULL or writable.
#[no_mangle]
pub unsafe extern "C" fn dgn_scene_layer(scene: *const DgnScene, index: usize, out: *mut DgnLayer) -> i32 {
    if scene.is_null() || out.is_null() { return 0; }
    let s = &*scene;
    let Some(layer) = s.scene.layers.get(index) else { return 0 };
    *out = DgnLayer {
        level: layer.level,
        name: s.names[index].as_ptr(),
        positions: layer.positions.as_ptr(),
        normals: layer.normals.as_ptr(),
        colors: layer.colors.as_ptr().cast(),
        vertex_count: layer.positions.len() / 3,
        indices: layer.indices.as_ptr(),
        index_count: layer.indices.len(),
    };
    1
}

/// Frees a scene from `dgn_scene`.
///
/// # Safety
/// `scene` must be NULL or a pointer from `dgn_scene` not yet freed.
#[no_mangle]
pub unsafe extern "C" fn dgn_scene_free(scene: *mut DgnScene) {
    if !scene.is_null() { drop(Box::from_raw(scene)); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dgn_read::synth::v8::*;

    fn sample() -> Vec<u8> {
        let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
        f.table(level_entry(70, "Walls"));
        f.graphic(element(3, 0x1000, 70, shown(0, 3), &doubles(&[0.0, 0.0, 2000.0, 0.0]), &[]));
        f.bytes()
    }

    #[test]
    fn errors_are_json() {
        let d = dgn_read::read(&sample()).unwrap();
        assert_eq!(query(&d, "nonsense")["ok"], false);
        assert_eq!(query(&d, "{}")["ok"], false);
        assert_eq!(query(&d, r#"{"query": "nope"}"#)["ok"], false);
        assert!(query(&d, r#"{"query": "elements", "model": 5}"#)["error"].as_str().unwrap().contains("no model 5"));
        let info = query(&d, r#"{"query": "info"}"#);
        assert_eq!(info["result"]["format"], "V8");
        let elements = query(&d, r#"{"query": "elements", "limit": 1}"#);
        assert_eq!(elements["result"]["total"], 1);
        assert_eq!(elements["result"]["elements"][0]["geometry"]["type"], "Line");
        assert_eq!(query(&d, r#"{"query": "levels"}"#)["result"][0]["name"], "Walls");
        assert_eq!(query(&d, r#"{"query": "drawing"}"#)["result"]["prims"][0]["polyline"][1][0], 2.0);
        assert_eq!(query(&d, r#"{"query": "models"}"#)["result"][0]["elements"], 1);
    }

    #[test]
    fn c_abi_round_trip() {
        unsafe {
            assert!(!CStr::from_ptr(dgn_version()).to_str().unwrap().is_empty());
            assert!(dgn_open(std::ptr::null()).is_null());
            let missing = CString::new("no/such/file.dgn").unwrap();
            assert!(dgn_open(missing.as_ptr()).is_null());
            assert!(CStr::from_ptr(dgn_last_error()).to_str().unwrap().contains("cannot read"));
            assert!(dgn_open_bytes(b"junk".as_ptr(), 4).is_null());
            assert!(CStr::from_ptr(dgn_last_error()).to_str().unwrap().contains("not a design file"));
            let req = CString::new(r#"{"query": "info"}"#).unwrap();
            let s = dgn_query(std::ptr::null(), req.as_ptr());
            assert!(CStr::from_ptr(s).to_str().unwrap().contains("\"ok\":false"));
            dgn_free(s);
            let bytes = sample();
            let doc = dgn_open_bytes(bytes.as_ptr(), bytes.len());
            assert!(!doc.is_null());
            let s = dgn_query(doc, req.as_ptr());
            assert!(CStr::from_ptr(s).to_str().unwrap().contains("\"ok\":true"));
            dgn_free(s);
            let scene = dgn_scene(doc, 0);
            dgn_close(doc);
            assert_eq!(dgn_scene_layer_count(scene), 1);
            let mut layer = std::mem::zeroed::<DgnLayer>();
            assert_eq!(dgn_scene_layer(scene, 0, &mut layer), 1);
            assert_eq!(CStr::from_ptr(layer.name).to_str().unwrap(), "Walls");
            assert_eq!((layer.level, layer.vertex_count, layer.index_count), (70, 6, 6));
            assert_eq!(*layer.colors, 255, "colour 3 is red");
            assert_eq!(dgn_scene_layer(scene, 1, &mut layer), 0);
            let mut origin = [1.0f64; 3];
            dgn_scene_origin(scene, origin.as_mut_ptr());
            assert_eq!(origin, [0.0; 3]);
            dgn_scene_free(scene);
        }
    }
}
