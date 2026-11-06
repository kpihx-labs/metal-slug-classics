use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(config: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metal-slug-classics"))
        .env("METAL_SLUG_CONFIG_HOME", config)
        .args(arguments)
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON ({error}): stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn create_profile(directory: &TempDir) {
    let output = run(
        directory.path(),
        &[
            "profile",
            "create",
            "default",
            "--runtime-compatibility-id",
            "retroarch-test-v1",
            "--runtime-sha256",
            &"1".repeat(64),
            "--core-compatibility-id",
            "fbneo-test-v1",
            "--core-sha256",
            &"2".repeat(64),
            "--controls-preset",
            "modern",
        ],
    );
    assert_success(&output);
}

#[test]
fn show_set_clear_reset_validate_import_export_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    create_profile(&directory);

    let output = run(directory.path(), &["--json", "controls", "show"]);
    assert!(output.status.success());
    assert_eq!(json(&output)["schema_version"], 1);

    let output = run(
        directory.path(),
        &[
            "--json",
            "controls",
            "set",
            "--player",
            "1",
            "--device",
            "pad",
            "--kind",
            "gamepad",
            "--index",
            "2",
            "--deadzone",
            "0",
            "--target",
            "hotkey.menu-toggle",
            "--binding",
            "button:8",
        ],
    );
    assert_success(&output);
    assert_eq!(json(&output)["valid"], true);

    let output = run(
        directory.path(),
        &[
            "controls",
            "set",
            "--player",
            "1",
            "--device",
            "pad",
            "--kind",
            "gamepad",
            "--index",
            "2",
            "--deadzone",
            "0",
            "--target",
            "hotkey.menu-toggle",
            "--clear",
        ],
    );
    assert_success(&output);

    let export = directory.path().join("portable-controls.json");
    let output = run(
        directory.path(),
        &["controls", "export", export.to_str().unwrap()],
    );
    assert_success(&output);
    assert!(export.is_file());

    let output = run(
        directory.path(),
        &["controls", "validate", "--input", export.to_str().unwrap()],
    );
    assert_success(&output);

    let output = run(directory.path(), &["controls", "reset"]);
    assert!(output.status.success());
    let output = run(directory.path(), &["controls", "validate"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("at least one player"));

    let output = run(
        directory.path(),
        &["controls", "import", export.to_str().unwrap()],
    );
    assert_success(&output);
    let output = run(directory.path(), &["controls", "validate"]);
    assert!(output.status.success());

    let profile = directory.path().join("profiles/default.json");
    let before = fs::read(&profile).unwrap();
    let output = run(
        directory.path(),
        &[
            "controls",
            "import",
            directory.path().join("missing.json").to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    assert_eq!(fs::read(profile).unwrap(), before);
}

#[test]
fn validate_reports_conflicts_from_import_without_modifying_profile() {
    let directory = tempfile::tempdir().unwrap();
    create_profile(&directory);
    let profile = directory.path().join("profiles/default.json");
    let before = fs::read(&profile).unwrap();
    let mut controls = json(&run(directory.path(), &["--json", "controls", "show"]));
    controls["players"][0]["devices"][0]["actions"]["fire-1"] =
        controls["players"][0]["devices"][0]["actions"]["up"].clone();
    let invalid = directory.path().join("invalid.json");
    fs::write(&invalid, serde_json::to_vec_pretty(&controls).unwrap()).unwrap();

    let output = run(
        directory.path(),
        &["controls", "import", invalid.to_str().unwrap()],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("one physical input"));
    assert_eq!(fs::read(profile).unwrap(), before);
}

#[test]
fn import_rejects_same_physical_key_across_logical_devices_and_fbneo_codes() {
    let directory = tempfile::tempdir().unwrap();
    create_profile(&directory);
    let profile = directory.path().join("profiles/default.json");
    let before = fs::read(&profile).unwrap();
    let mut controls = json(&run(directory.path(), &["--json", "controls", "show"]));
    controls["players"][0]["devices"][0]["actions"]["up"]["fbneo_code"] = Value::from(0x1111);
    let mut second = controls["players"][0]["devices"][0].clone();
    second["id"] = Value::from("other-logical-id");
    second["actions"] = serde_json::json!({
        "fire-1": {
            "type": "key",
            "key": "W",
            "fbneo_code": 0x2222
        }
    });
    controls["players"][0]["devices"]
        .as_array_mut()
        .unwrap()
        .push(second);
    let invalid = directory.path().join("physical-conflict.json");
    fs::write(&invalid, serde_json::to_vec_pretty(&controls).unwrap()).unwrap();

    let output = run(
        directory.path(),
        &["--json", "controls", "import", invalid.to_str().unwrap()],
    );
    assert!(!output.status.success());
    assert_eq!(output.stdout, Vec::<u8>::new());
    let failure: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(failure["error"]["code"], "controls-failed");
    assert!(
        failure["error"]["message"]
            .as_str()
            .unwrap()
            .contains("one physical input")
    );
    assert_eq!(fs::read(profile).unwrap(), before);
}

#[test]
fn oversized_import_is_rejected_without_replacing_profile() {
    let directory = tempfile::tempdir().unwrap();
    create_profile(&directory);
    let profile = directory.path().join("profiles/default.json");
    let before = fs::read(&profile).unwrap();
    let oversized = directory.path().join("oversized.json");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(metal_slug_classics::controls::MAX_CONTROLS_IMPORT_BYTES + 1)
        .unwrap();

    let output = run(
        directory.path(),
        &["controls", "import", oversized.to_str().unwrap()],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds"));
    assert_eq!(fs::read(profile).unwrap(), before);
}
