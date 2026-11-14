use std::path::{Path, PathBuf};
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_vistaforge")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vf-cli-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(binary())
        .args(args)
        .output()
        .expect("failed to execute vistaforge")
}

fn assert_png(path: &Path) {
    let bytes =
        std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    assert!(
        bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "{} is not a PNG",
        path.display()
    );
    assert!(
        bytes.len() > 100,
        "{} is suspiciously small",
        path.display()
    );
}

#[test]
fn generate_info_and_render_project() {
    let dir = temp_dir("project");
    let project = dir.join("scene.vfproj");
    let preview = dir.join("preview.png");
    let output = run(&[
        "terrain",
        "generate",
        "--mode",
        "classic",
        "--size",
        "65",
        "--seed",
        "5",
        "--name",
        "CLI test",
        "--out",
        project.to_str().unwrap(),
        "--render",
        preview.to_str().unwrap(),
        "--render-width",
        "64",
        "--render-height",
        "48",
    ]);
    assert!(
        output.status.success(),
        "generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.exists());
    assert_png(&preview);

    let info = run(&["project", "info", project.to_str().unwrap(), "--json"]);
    assert!(info.status.success());
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(json["dimensions"][0], 65);
    assert_eq!(json["seed"], 5);
    assert_eq!(json["source"], "classic");

    let second = dir.join("second.png");
    let render = run(&[
        "render",
        project.to_str().unwrap(),
        second.to_str().unwrap(),
        "--width",
        "32",
        "--height",
        "24",
        "--preview",
        "--no-clouds",
        "--no-water",
        "--json",
    ]);
    assert!(
        render.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&render.stderr)
    );
    assert_png(&second);
    let report: serde_json::Value = serde_json::from_slice(&render.stdout).unwrap();
    assert_eq!(report["width"], 32);
    assert_eq!(report["height"], 24);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn modifier_stack_round_trip() {
    let dir = temp_dir("modifiers");
    let project = dir.join("scene.vfproj");
    let generated = run(&[
        "terrain",
        "generate",
        "--mode",
        "classic",
        "--size",
        "65",
        "--seed",
        "9",
        "--name",
        "Modifier CLI",
        "--out",
        project.to_str().unwrap(),
    ]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let add = run(&[
        "terrain",
        "modifier",
        "add",
        project.to_str().unwrap(),
        "displace",
        "--param",
        "amount_m=100",
        "--param",
        "scale_m=400",
    ]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = run(&[
        "terrain",
        "modifier",
        "list",
        project.to_str().unwrap(),
        "--json",
    ]);
    assert!(list.status.success());
    let stack: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(stack["count"], 1);
    assert_eq!(stack["items"][0]["kind"], "displace");
    let amount = stack["items"][0]["params"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["key"] == "amount_m")
        .unwrap()["value"]
        .as_f64()
        .unwrap();
    assert!((amount - 100.0).abs() < 1e-3);

    let crash = run(&[
        "terrain",
        "modifier",
        "add",
        project.to_str().unwrap(),
        "crater",
        "--param",
        "count=2",
    ]);
    assert!(crash.status.success());
    let set = run(&[
        "terrain",
        "modifier",
        "set",
        project.to_str().unwrap(),
        "0",
        "--param",
        "amount_m=150",
        "--disable",
    ]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );

    let info = run(&["terrain", "info", project.to_str().unwrap(), "--json"]);
    assert!(info.status.success());
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(json["modifiers"]["count"], 2);
    assert_eq!(json["modifiers"]["enabled"], 1);

    let preview = dir.join("preview.png");
    let rendered = run(&[
        "render",
        project.to_str().unwrap(),
        preview.to_str().unwrap(),
        "--preview",
        "--width",
        "64",
        "--height",
        "48",
    ]);
    assert!(
        rendered.status.success(),
        "{}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    assert_png(&preview);

    let bake = run(&["terrain", "modifier", "bake", project.to_str().unwrap()]);
    assert!(
        bake.status.success(),
        "{}",
        String::from_utf8_lossy(&bake.stderr)
    );
    let info = run(&["terrain", "info", project.to_str().unwrap(), "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(json["modifiers"]["count"], 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_files_fail_cleanly() {
    let output = run(&["project", "info", "/definitely/not/here.vfproj"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("error:"), "unexpected stderr: {stderr}");
}

#[test]
fn dem_roundtrip_and_import_when_corpus_is_present() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../re/unpacked/disk03/ALPS.DE");
    if !corpus.exists() {
        eprintln!("skipping: corpus not present");
        return;
    }
    let dir = temp_dir("dem");
    let roundtrip = run(&["dem", "roundtrip", corpus.to_str().unwrap()]);
    assert!(
        roundtrip.status.success(),
        "roundtrip failed: {}",
        String::from_utf8_lossy(&roundtrip.stderr)
    );
    assert!(String::from_utf8_lossy(&roundtrip.stdout).contains("OK:"));

    let project = dir.join("alps.vfproj");
    let result = run(&[
        "terrain",
        "import",
        corpus.to_str().unwrap(),
        "--out",
        project.to_str().unwrap(),
    ]);
    assert!(
        result.status.success(),
        "import failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let info = run(&["project", "info", project.to_str().unwrap(), "--json"]);
    assert!(info.status.success());
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(json["dimensions"][0], 258);
    assert_eq!(json["source"], "dem");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn anim_still_renders_a_flight_frame() {
    let dir = temp_dir("anim");
    let project = dir.join("scene.vfproj");
    let generated = run(&[
        "terrain",
        "generate",
        "--mode",
        "modern",
        "--size",
        "258",
        "--seed",
        "7",
        "--preset",
        "alpine",
        "--rock",
        "limestone",
        "--name",
        "flight test",
        "--out",
        project.to_str().unwrap(),
    ]);
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let output = dir.join("frame.png");
    let still = run(&[
        "anim",
        "still",
        project.to_str().unwrap(),
        output.to_str().unwrap(),
        "--preset",
        "cinematic",
        "--time",
        "1.5",
        "--width",
        "96",
        "--height",
        "64",
    ]);
    assert!(
        still.status.success(),
        "{}",
        String::from_utf8_lossy(&still.stderr)
    );
    assert_png(&output);
}
