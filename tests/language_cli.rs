use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn run(config: &Path, work: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_metal-slug-classics"))
        .env("METAL_SLUG_CONFIG_HOME", config)
        .env("METAL_SLUG_WORK", work)
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

fn create_profile(directory: &TempDir, work: &Path) {
    let output = run(
        directory.path(),
        work,
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
        ],
    );
    assert_success(&output);
}

#[test]
fn language_list_show_set_validate_import_export_round_trip_is_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path().join("work");
    fs::create_dir_all(&work).unwrap();
    create_profile(&directory, &work);

    let listed = run(directory.path(), &work, &["--json", "language", "list"]);
    assert_success(&listed);
    let listed = json(&listed);
    assert_eq!(listed.as_array().unwrap().len(), 2);
    assert_eq!(listed[0]["launcher_coverage_percent"], 100);
    assert_eq!(listed[1]["launcher_coverage_percent"], 100);

    let set = run(
        directory.path(),
        &work,
        &[
            "--json", "language", "set", "--locale", "fr", "--region", "europe", "--system", "aes",
        ],
    );
    assert_success(&set);
    assert_eq!(json(&set)["language"]["locale"], "fr");

    let shown = run(directory.path(), &work, &["--json", "language", "show"]);
    assert_success(&shown);
    let shown = json(&shown);
    assert_eq!(shown["language"]["schema_version"], 1);
    assert_eq!(shown["language"]["fallback_locale"], "en");
    assert_eq!(shown["region"], "europe");
    assert_eq!(shown["system"], "aes");

    let export = directory.path().join("language.json");
    let exported = run(
        directory.path(),
        &work,
        &["language", "export", export.to_str().unwrap()],
    );
    assert_success(&exported);
    assert_success(&run(
        directory.path(),
        &work,
        &["language", "validate", "--input", export.to_str().unwrap()],
    ));

    let profile_path = directory.path().join("profiles/default.json");
    let before = fs::read(&profile_path).unwrap();
    let invalid = directory.path().join("invalid-language.json");
    fs::write(
        &invalid,
        br#"{"schema_version":1,"locale":"fr","fallback_locale":"fr","translation_pack_sha256":null}"#,
    )
    .unwrap();
    let rejected = run(
        directory.path(),
        &work,
        &["language", "import", invalid.to_str().unwrap()],
    );
    assert!(!rejected.status.success());
    assert_eq!(fs::read(&profile_path).unwrap(), before);
    fs::write(
        &invalid,
        br#"{"schema_version":1,"locale":"fr_FR","fallback_locale":"en","translation_pack_sha256":null}"#,
    )
    .unwrap();
    let rejected = run(
        directory.path(),
        &work,
        &["language", "import", invalid.to_str().unwrap()],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("BCP 47"));
    assert_eq!(fs::read(&profile_path).unwrap(), before);

    assert_success(&run(
        directory.path(),
        &work,
        &["language", "set", "--locale", "en"],
    ));
    assert_success(&run(
        directory.path(),
        &work,
        &["language", "import", export.to_str().unwrap()],
    ));
    assert_eq!(
        json(&run(
            directory.path(),
            &work,
            &["--json", "language", "show"]
        ))["language"]["locale"],
        "fr"
    );
}

#[test]
fn historical_profile_without_language_migrates_on_next_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path().join("work");
    fs::create_dir_all(&work).unwrap();
    create_profile(&directory, &work);
    let profile_path = directory.path().join("profiles/default.json");
    let mut profile: Value = serde_json::from_slice(&fs::read(&profile_path).unwrap()).unwrap();
    profile.as_object_mut().unwrap().remove("language");
    fs::write(&profile_path, serde_json::to_vec_pretty(&profile).unwrap()).unwrap();

    let shown = run(directory.path(), &work, &["--json", "language", "show"]);
    assert_success(&shown);
    assert_eq!(json(&shown)["language"]["locale"], "en");
    assert_success(&run(
        directory.path(),
        &work,
        &["language", "set", "--locale", "fr"],
    ));
    let persisted: Value = serde_json::from_slice(&fs::read(profile_path).unwrap()).unwrap();
    assert_eq!(persisted["language"]["schema_version"], 1);
    assert_eq!(persisted["language"]["locale"], "fr");
}

#[test]
fn pack_cli_refuses_incomplete_and_wrong_base_without_partial_output() {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let base = work.join("base.rom");
    let translated = work.join("translated.rom");
    fs::write(&base, b"owned English bytes").unwrap();
    fs::write(&translated, b"owned French! bytes").unwrap();
    let pack = work.join("fr.pack.json");

    let incomplete = run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "generate",
            base.to_str().unwrap(),
            translated.to_str().unwrap(),
            "--output",
            pack.to_str().unwrap(),
            "--game",
            "mslug",
            "--locale",
            "fr",
            "--coverage",
            "99",
        ],
    );
    assert!(!incomplete.status.success());
    assert!(!pack.exists());

    let generated = run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "generate",
            base.to_str().unwrap(),
            translated.to_str().unwrap(),
            "--output",
            pack.to_str().unwrap(),
            "--game",
            "mslug",
            "--locale",
            "fr",
        ],
    );
    assert_success(&generated);
    assert_success(&run(
        directory.path(),
        &work,
        &["language", "pack", "validate", pack.to_str().unwrap()],
    ));

    let wrong = work.join("wrong.rom");
    fs::write(&wrong, b"wrong base contents!").unwrap();
    let output = work.join("should-not-exist.rom");
    let rejected = run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "apply",
            pack.to_str().unwrap(),
            wrong.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ],
    );
    assert!(!rejected.status.success());
    assert!(!output.exists());

    let applied = work.join("applied.rom");
    assert_success(&run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "apply",
            pack.to_str().unwrap(),
            base.to_str().unwrap(),
            "--output",
            applied.to_str().unwrap(),
        ],
    ));
    assert_eq!(fs::read(&applied).unwrap(), fs::read(&translated).unwrap());
    let reverted = work.join("reverted.rom");
    assert_success(&run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "revert",
            pack.to_str().unwrap(),
            applied.to_str().unwrap(),
            "--output",
            reverted.to_str().unwrap(),
        ],
    ));
    assert_eq!(fs::read(reverted).unwrap(), fs::read(base).unwrap());
}

#[test]
fn pack_cli_rejects_traversal_complete_payload_and_oversized_input() {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let base = work.join("base.rom");
    let translated = work.join("translated.rom");
    fs::write(&base, [0_u8; 32]).unwrap();
    fs::write(&translated, [1_u8; 32]).unwrap();

    let escaped = directory.path().join("escaped/pack.json");
    let rejected = run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "generate",
            base.to_str().unwrap(),
            translated.to_str().unwrap(),
            "--output",
            "../escaped/pack.json",
            "--game",
            "mslug",
            "--locale",
            "fr",
        ],
    );
    assert!(!rejected.status.success());
    assert!(!escaped.exists());

    let complete = work.join("complete.pack.json");
    let rejected = run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "generate",
            base.to_str().unwrap(),
            translated.to_str().unwrap(),
            "--output",
            complete.to_str().unwrap(),
            "--game",
            "mslug",
            "--locale",
            "fr",
        ],
    );
    assert!(!rejected.status.success());
    assert!(!complete.exists());

    let oversized = work.join("oversized.rom");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let oversized_pack = work.join("oversized.pack.json");
    let rejected = run(
        directory.path(),
        &work,
        &[
            "language",
            "pack",
            "generate",
            oversized.to_str().unwrap(),
            translated.to_str().unwrap(),
            "--output",
            oversized_pack.to_str().unwrap(),
            "--game",
            "mslug",
            "--locale",
            "fr",
        ],
    );
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("exceeds"));
}
