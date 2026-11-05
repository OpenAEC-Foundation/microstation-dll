//! The command line tool on a V8 file written in the test.

use std::process::Command;

use dgn_read::synth::v8::*;

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_dgn")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(if out.status.success() { &out.stdout } else { &out.stderr }).into_owned())
}

#[test]
fn info_dump_tree_and_obj() {
    let dir = std::env::temp_dir().join(format!("dgn-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("square.dgn");
    let mut f = File::new(1000.0, 1.0, 1.0, [0.0; 3]);
    f.table(level_entry(70, "Walls"));
    f.graphic(element(6, 0x1000, 70, shown(D3, 0), &counted(5, &[0.0, 0.0, 0.0, 1000.0, 0.0, 0.0, 1000.0, 1000.0, 0.0, 0.0, 1000.0, 0.0, 0.0, 0.0, 0.0]), &[]));
    std::fs::write(&path, f.bytes()).unwrap();
    let file = path.to_str().unwrap();

    let (ok, info) = run(&["info", file]);
    assert!(ok, "{info}");
    let info: serde_json::Value = serde_json::from_str(&info).unwrap();
    assert_eq!((info["format"].as_str(), info["elements"]["Shape"].as_u64()), (Some("V8"), Some(1)));

    let (ok, dump) = run(&["dump", file]);
    assert!(ok && dump.contains("Dgn-Md/#000000/Dgn^G: 1 elements") && dump.contains("\"Walls\""), "{dump}");
    let (ok, tree) = run(&["tree", file]);
    assert!(ok && tree.contains("Dgn-Md/#000000/Dgn~Mh"), "{tree}");

    let obj = dir.join("square.obj");
    let (ok, message) = run(&["obj", file, "-o", obj.to_str().unwrap()]);
    assert!(ok, "{message}");
    let text = std::fs::read_to_string(&obj).unwrap();
    assert!(text.contains("g Walls") && text.lines().filter(|l| l.starts_with("f ")).count() == 2, "{text}");

    let (ok, message) = run(&["info", dir.join("missing.dgn").to_str().unwrap()]);
    assert!(!ok && message.contains("cannot read"), "{message}");
    let (ok, message) = run(&["nonsense"]);
    assert!(!ok && message.contains("usage"), "{message}");
    std::fs::remove_dir_all(&dir).ok();
}
