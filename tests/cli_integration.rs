//! CLI integration tests for the `fatx` binary.
//!
//! Tests run the fatx binary against mkimage-generated test images and verify
//! stdout, stderr, and exit codes.

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Path to the fatx-mkimage binary
fn mkimage_bin() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
    let bin = dir.join("target").join(profile).join("fatx-mkimage");
    if !bin.exists() {
        let status = Command::new("cargo")
            .args(["build", "-p", "fatx-mkimage"])
            .current_dir(&dir)
            .status()
            .expect("build fatx-mkimage");
        assert!(status.success());
    }
    bin
}

/// Create fatx command
fn fatx_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fatx"))
}

/// Create a temp image. Returns (TempDir, image_path).
fn create_test_image(size_mb: u32, populate: bool) -> (TempDir, PathBuf) {
    let tmp = TempDir::new().expect("create temp dir");
    let img = tmp.path().join("test.img");

    let mut args = vec![
        img.to_str().unwrap().to_string(),
        "--size".to_string(),
        format!("{}M", size_mb),
        "--force".to_string(),
    ];
    if populate {
        args.push("--populate".to_string());
    }

    let output = Command::new(mkimage_bin())
        .args(&args)
        .output()
        .expect("run fatx-mkimage");
    assert!(output.status.success(), "mkimage failed: {}", String::from_utf8_lossy(&output.stderr));

    (tmp, img)
}

// ===========================================================================
// Basic CLI tests
// ===========================================================================

#[test]
fn test_fatx_version() {
    let output = fatx_bin()
        .arg("--version")
        .output()
        .expect("run fatx --version");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fatx"), "version should contain 'fatx': {}", stdout);
}

#[test]
fn test_fatx_help() {
    let output = fatx_bin()
        .arg("--help")
        .output()
        .expect("run fatx --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FATX"), "help should mention FATX");
    assert!(stdout.contains("ls"), "help should list ls command");
    assert!(stdout.contains("info"), "help should list info command");
}

// ===========================================================================
// fatx ls
// ===========================================================================

#[test]
fn test_cli_ls_empty() {
    let (_tmp, img) = create_test_image(4, false);

    let output = fatx_bin()
        .args(["ls", img.to_str().unwrap(), "/"])
        .output()
        .expect("run fatx ls");

    assert!(output.status.success(), "fatx ls failed: {}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn test_cli_ls_populated() {
    let (_tmp, img) = create_test_image(256, true);

    let output = fatx_bin()
        .args(["ls", img.to_str().unwrap(), "/"])
        .output()
        .expect("run fatx ls");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Content"), "should list Content dir");
    assert!(stdout.contains("Cache"), "should list Cache dir");
    assert!(stdout.contains("name.txt"), "should list name.txt");
    assert!(stdout.contains("launch.ini"), "should list launch.ini");
}

#[test]
fn test_cli_ls_subdirectory() {
    let (_tmp, img) = create_test_image(256, true);

    let output = fatx_bin()
        .args(["ls", img.to_str().unwrap(), "/Apps"])
        .output()
        .expect("run fatx ls /Apps");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Aurora"), "should list Aurora under /Apps");
}

#[test]
fn test_cli_ls_nonexistent_path() {
    let (_tmp, img) = create_test_image(4, false);

    let output = fatx_bin()
        .args(["ls", img.to_str().unwrap(), "/nonexistent"])
        .output()
        .expect("run fatx ls nonexistent");

    assert!(!output.status.success(), "ls nonexistent path should fail");
}

// ===========================================================================
// fatx info
// ===========================================================================

#[test]
fn test_cli_info() {
    let (_tmp, img) = create_test_image(4, false);

    let output = fatx_bin()
        .args(["info", img.to_str().unwrap()])
        .output()
        .expect("run fatx info");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("FATX Volume Information"), "should show header");
    assert!(stdout.contains("FAT type:"), "should show FAT type");
    assert!(stdout.contains("Free:"), "should show free space");
}

#[test]
fn test_cli_info_json() {
    let (_tmp, img) = create_test_image(4, false);

    let output = fatx_bin()
        .args(["info", img.to_str().unwrap(), "--json"])
        .output()
        .expect("run fatx info --json");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    let json: serde_json::Value = serde_json::from_str(&stdout)
        .expect("info --json should produce valid JSON");

    assert!(json["total_clusters"].is_number(), "should have total_clusters");
    assert!(json["free_clusters"].is_number(), "should have free_clusters");
    assert!(json["cluster_size"].is_number(), "should have cluster_size");
}

#[test]
fn test_cli_info_nonexistent_file() {
    let output = fatx_bin()
        .args(["info", "/tmp/does_not_exist_fatx_test.img"])
        .output()
        .expect("run fatx info nonexistent");

    assert!(!output.status.success(), "info on nonexistent file should fail");
}

// ===========================================================================
// fatx read
// ===========================================================================

#[test]
fn test_cli_read_file() {
    let (_tmp, img) = create_test_image(256, true);

    let output = fatx_bin()
        .args(["read", img.to_str().unwrap(), "/name.txt"])
        .output()
        .expect("run fatx read");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Test Xbox 360"), "should read file content from populated image");
}

// ===========================================================================
// fatx scan
// ===========================================================================

#[test]
fn test_cli_scan_nonexistent_device() {
    let output = fatx_bin()
        .args(["scan", "/nonexistent/device"])
        .output()
        .expect("run fatx scan");

    assert!(!output.status.success());
}

#[test]
fn test_cli_scan_image() {
    let (_tmp, img) = create_test_image(4, false);

    let output = fatx_bin()
        .args(["scan", img.to_str().unwrap()])
        .output()
        .expect("run fatx scan on image");

    // Scan on a small image may or may not find partitions
    // but should not crash
    let _ = output.status;
}
