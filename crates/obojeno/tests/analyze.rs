//! `obojeno analyze` and `obojeno decode-pixels`, the helpers the daemon
//! runs for decoding images.

use std::path::Path;
use std::process::Command;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/lagoon.heic");

#[test]
fn analyze_prints_the_color_and_writes_the_thumbnail() {
    let dir = tempfile::tempdir().unwrap();
    let thumbnail = dir.path().join("thumb.jpg");
    let output = Command::new(env!("CARGO_BIN_EXE_obojeno"))
        .args(["analyze", FIXTURE, "--thumbnail"])
        .arg(&thumbnail)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let analysis: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let source = analysis["source"].as_str().unwrap();
    assert!(source.starts_with('#') && source.len() == 7, "{source}");
    assert_eq!(analysis["key"].as_str().unwrap().len(), 64, "a SHA-256");
    let thumb = image::open(&thumbnail).unwrap();
    assert_eq!(
        (thumb.width(), thumb.height()),
        (160, 100),
        "never scaled up"
    );
}

#[test]
fn analyze_fails_cleanly_on_non_images() {
    let output = Command::new(env!("CARGO_BIN_EXE_obojeno"))
        .args(["analyze"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("macOS cannot read it"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn decode_pixels_writes_them_and_prints_their_size() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("pixels");
    let output = Command::new(env!("CARGO_BIN_EXE_obojeno"))
        .args(["decode-pixels", FIXTURE, "--bgra", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "160 100");
    assert_eq!(std::fs::metadata(&out).unwrap().len(), 160 * 100 * 4);
}
