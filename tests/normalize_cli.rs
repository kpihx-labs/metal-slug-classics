use metal_slug_classics::audit::sha256_file;
use metal_slug_classics::manifest::{SetManifest, manifests};
use serde_json::Value;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn run(arguments: &[&Path]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metal-slug-classics"));
    command.arg("normalize");
    for argument in arguments {
        command.arg(argument);
    }
    command.output().unwrap()
}

fn run_with_flags(source: &Path, output: &Path, flags: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_metal-slug-classics"));
    command
        .arg("normalize")
        .arg(source)
        .arg("--output")
        .arg(output)
        .args(flags)
        .output()
        .unwrap()
}

fn fixture_manifest() -> SetManifest {
    manifests()
        .into_iter()
        .find(|manifest| manifest.id == "mslug-modern")
        .unwrap()
}

fn bytes_with_crc(size: u64, target: u32) -> Vec<u8> {
    let size = usize::try_from(size).unwrap();
    assert!(size >= 4);
    let mut bytes = vec![0_u8; size];
    let patch_start = size - 4;
    let base = crc32fast::hash(&bytes);
    let zero_crc = crc32fast::hash(&[0_u8; 4]);
    let mut columns = [0_u32; 32];
    for bit in 0..32 {
        let mut patch = [0_u8; 4];
        patch[bit / 8] = 1 << (bit % 8);
        columns[bit] = crc32fast::hash(&patch) ^ zero_crc;
    }
    let mut rows = [(0_u32, false); 32];
    for (row, item) in rows.iter_mut().enumerate() {
        let mut coefficients = 0_u32;
        for (column, delta) in columns.iter().enumerate() {
            if (delta >> row) & 1 == 1 {
                coefficients |= 1 << column;
            }
        }
        *item = (coefficients, ((target ^ base) >> row) & 1 == 1);
    }
    for column in 0..32 {
        let pivot = (column..32)
            .find(|row| (rows[*row].0 >> column) & 1 == 1)
            .expect("CRC influence matrix is invertible");
        rows.swap(column, pivot);
        for row in 0..32 {
            if row != column && (rows[row].0 >> column) & 1 == 1 {
                rows[row].0 ^= rows[column].0;
                rows[row].1 ^= rows[column].1;
            }
        }
    }
    let mut patch = 0_u32;
    for (bit, (_, value)) in rows.iter().enumerate() {
        if *value {
            patch |= 1 << bit;
        }
    }
    bytes[patch_start..].copy_from_slice(&patch.to_le_bytes());
    assert_eq!(crc32fast::hash(&bytes), target);
    bytes
}

fn write_zip(path: &Path, members: &[(String, Vec<u8>)]) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in members {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap();
}

fn write_zip_with_special_entry(
    path: &Path,
    members: &[(String, Vec<u8>)],
    special_name: &str,
    mode: Option<u32>,
) {
    let file = File::create(path).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in members {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    if let Some(mode) = mode {
        archive
            .start_file(special_name, options.unix_permissions(mode))
            .unwrap();
        archive.write_all(b"special").unwrap();
    } else {
        archive.add_directory(special_name, options).unwrap();
    }
    archive.finish().unwrap();
}

fn rewrite_zip_member_name(path: &Path, old: &str, new: &str) {
    assert_eq!(old.len(), new.len());
    let mut bytes = fs::read(path).unwrap();
    let mut replacements = 0;
    for offset in 0..=bytes.len() - old.len() {
        if bytes[offset..offset + old.len()] == *old.as_bytes() {
            bytes[offset..offset + new.len()].copy_from_slice(new.as_bytes());
            replacements += 1;
        }
    }
    assert_eq!(replacements, 2);
    fs::write(path, bytes).unwrap();
}

fn mark_zip_member_as_symlink(path: &Path, name: &str) {
    let mut bytes = fs::read(path).unwrap();
    let mut offset = 0;
    let mut found = false;
    while offset + 46 <= bytes.len() {
        if bytes[offset..offset + 4] != [0x50, 0x4b, 0x01, 0x02] {
            offset += 1;
            continue;
        }
        let name_length = usize::from(u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]));
        let extra_length =
            usize::from(u16::from_le_bytes([bytes[offset + 30], bytes[offset + 31]]));
        let comment_length =
            usize::from(u16::from_le_bytes([bytes[offset + 32], bytes[offset + 33]]));
        let name_start = offset + 46;
        if name_start + name_length > bytes.len() {
            break;
        }
        if bytes[name_start..name_start + name_length] == *name.as_bytes() {
            bytes[offset + 5] = 3;
            bytes[offset + 38..offset + 42].copy_from_slice(&(0o120_777_u32 << 16).to_le_bytes());
            found = true;
            break;
        }
        offset = name_start + name_length + extra_length + comment_length;
    }
    assert!(found);
    fs::write(path, bytes).unwrap();
}

fn write_valid_bios(directory: &Path) {
    let signatures = [
        ("fixed-a.bin", 0x02_0000, 0x9441_6d67),
        ("fixed-b.bin", 0x02_0000, 0xc2ea_0cfd),
        ("fixed-c.bin", 0x02_0000, 0x5a86_cff2),
        ("system.bin", 0x02_0000, 0x91b6_4be3),
    ];
    let members = signatures
        .into_iter()
        .map(|(name, size, crc)| (name.to_owned(), bytes_with_crc(size, crc)))
        .collect::<Vec<_>>();
    write_zip(&directory.join("neogeo.zip"), &members);
}

fn write_game(path: &Path, manifest: &SetManifest, rename: bool, traversal: bool) {
    let members = manifest
        .members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            let name = if index == 0 && traversal {
                format!("../{}", member.name)
            } else if index == 0 && rename {
                "renamed-first-member.bin".to_owned()
            } else {
                member.name.to_owned()
            };
            (name, bytes_with_crc(member.size, member.crc32))
        })
        .collect::<Vec<_>>();
    write_zip(path, &members);
}

fn read_all(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

#[test]
fn normalizes_immutably_and_deterministically_with_provenance() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    write_valid_bios(&source);
    let manifest = fixture_manifest();
    let source_zip = source.join("mslug-owned.zip");
    write_game(&source_zip, &manifest, true, false);
    let source_before = sha256_file(&source_zip).unwrap();
    let bios_before = sha256_file(&source.join("neogeo.zip")).unwrap();

    let relocated_source = temporary.path().join("relocated-source");
    fs::create_dir(&relocated_source).unwrap();
    fs::copy(&source_zip, relocated_source.join("mslug-owned.zip")).unwrap();
    fs::copy(
        source.join("neogeo.zip"),
        relocated_source.join("neogeo.zip"),
    )
    .unwrap();

    let output_a = temporary.path().join("normalized-a");
    let output_b = temporary.path().join("normalized-b");
    let first = run_with_flags(&source, &output_a, &[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let second = run_with_flags(&relocated_source, &output_b, &[]);
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    assert_eq!(sha256_file(&source_zip).unwrap(), source_before);
    assert_eq!(
        sha256_file(&source.join("neogeo.zip")).unwrap(),
        bios_before
    );
    assert_eq!(
        read_all(&output_a.join("mslug.zip")),
        read_all(&output_b.join("mslug.zip"))
    );
    assert_eq!(
        read_all(&output_a.join("mslug.zip.provenance.json")),
        read_all(&output_b.join("mslug.zip.provenance.json"))
    );
    let provenance_path = output_a.join("mslug.zip.provenance.json");
    let provenance: Value = serde_json::from_slice(&read_all(&provenance_path)).unwrap();
    assert_eq!(provenance["schema_version"], 3);
    assert_eq!(provenance["bios_required"], true);
    assert!(provenance.get("source_path").is_none());
    assert_eq!(provenance["manifest_id"], "mslug-modern");
    assert_eq!(provenance["source_sha256"], source_before);
    assert_eq!(
        provenance["members"][0]["source_name"],
        "renamed-first-member.bin"
    );
    assert_eq!(provenance["members"][0]["target_name"], "201-p1.p1");
    assert_eq!(
        provenance["output_sha256"],
        sha256_file(&output_a.join("mslug.zip")).unwrap()
    );

    let mut archive = ZipArchive::new(File::open(output_a.join("mslug.zip")).unwrap()).unwrap();
    let mut source_archive = ZipArchive::new(File::open(&source_zip).unwrap()).unwrap();
    assert_eq!(archive.len(), manifest.members.len());
    for (index, expected) in manifest.members.iter().enumerate() {
        let mut member = archive.by_index(index).unwrap();
        assert_eq!(member.name(), expected.name);
        assert_eq!(member.size(), expected.size);
        assert_eq!(member.crc32(), expected.crc32);
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        assert_eq!(crc32fast::hash(&bytes), expected.crc32);
        let source_name = if index == 0 {
            "renamed-first-member.bin"
        } else {
            expected.name
        };
        let mut source_member = source_archive.by_name(source_name).unwrap();
        let mut source_bytes = Vec::new();
        source_member.read_to_end(&mut source_bytes).unwrap();
        assert_eq!(bytes, source_bytes);
    }

    let existing_hash = sha256_file(&output_a.join("mslug.zip")).unwrap();
    let refused = run_with_flags(&source, &output_a, &[]);
    assert!(!refused.status.success());
    assert_eq!(
        sha256_file(&output_a.join("mslug.zip")).unwrap(),
        existing_hash
    );
    let replaced = run_with_flags(&source, &output_a, &["--force"]);
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(
        sha256_file(&output_a.join("mslug.zip")).unwrap(),
        existing_hash
    );
}

#[test]
fn refuses_damaged_and_unknown_inputs_but_normalizes_complete_roms_without_bios() {
    let temporary = tempfile::tempdir().unwrap();
    let manifest = fixture_manifest();

    let incomplete = temporary.path().join("incomplete");
    fs::create_dir(&incomplete).unwrap();
    write_valid_bios(&incomplete);
    let two_valid = manifest.members[..2]
        .iter()
        .map(|member| {
            (
                member.name.to_owned(),
                bytes_with_crc(member.size, member.crc32),
            )
        })
        .collect::<Vec<_>>();
    write_zip(&incomplete.join("mslug-incomplete.zip"), &two_valid);
    let incomplete_output = temporary.path().join("incomplete-output");
    assert!(
        !run_with_flags(&incomplete, &incomplete_output, &[])
            .status
            .success()
    );
    assert!(!incomplete_output.exists());

    let corrupt = temporary.path().join("corrupt");
    fs::create_dir(&corrupt).unwrap();
    write_valid_bios(&corrupt);
    let corrupt_members = manifest.members[..2]
        .iter()
        .map(|member| {
            (
                member.name.to_owned(),
                vec![0_u8; usize::try_from(member.size).unwrap()],
            )
        })
        .collect::<Vec<_>>();
    write_zip(&corrupt.join("mslug-corrupt.zip"), &corrupt_members);
    let corrupt_output = temporary.path().join("corrupt-output");
    assert!(
        !run_with_flags(&corrupt, &corrupt_output, &[])
            .status
            .success()
    );
    assert!(!corrupt_output.exists());

    let unknown = temporary.path().join("unknown.zip");
    write_zip(&unknown, &[("unknown.bin".to_owned(), vec![1, 2, 3])]);
    let unknown_output = temporary.path().join("unknown-output");
    assert!(
        !run_with_flags(&unknown, &unknown_output, &[])
            .status
            .success()
    );
    assert!(!unknown_output.exists());

    let no_bios = temporary.path().join("no-bios");
    fs::create_dir(&no_bios).unwrap();
    write_game(&no_bios.join("mslug-complete.zip"), &manifest, false, false);
    let no_bios_output = temporary.path().join("no-bios-output");
    let no_bios_result = run_with_flags(&no_bios, &no_bios_output, &[]);
    assert!(
        no_bios_result.status.success(),
        "{}",
        String::from_utf8_lossy(&no_bios_result.stderr)
    );
    let provenance: Value =
        serde_json::from_slice(&read_all(&no_bios_output.join("mslug.zip.provenance.json")))
            .unwrap();
    assert_eq!(provenance["bios_required"], true);
}

#[test]
fn skip_blocked_reports_corrupt_sets_and_publishes_complete_sets() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("collection");
    fs::create_dir(&source).unwrap();
    let manifest = fixture_manifest();
    write_game(&source.join("mslug-complete.zip"), &manifest, false, false);
    let incomplete = manifest.members[..2]
        .iter()
        .map(|member| {
            (
                member.name.to_owned(),
                bytes_with_crc(member.size, member.crc32),
            )
        })
        .collect::<Vec<_>>();
    write_zip(&source.join("mslug-incomplete.zip"), &incomplete);
    let output = temporary.path().join("normalized");

    let result = run_with_flags(&source, &output, &["--skip-blocked"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(output.join("mslug.zip").is_file());
    assert!(String::from_utf8_lossy(&result.stdout).contains("skipped"));
}

#[test]
fn refuses_traversal_member_and_symlink_source() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    write_valid_bios(&source);
    write_game(
        &source.join("mslug-traversal.zip"),
        &fixture_manifest(),
        false,
        true,
    );
    let output = temporary.path().join("traversal-output");
    assert!(!run_with_flags(&source, &output, &[]).status.success());
    assert!(!output.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = temporary.path().join("source-link");
        symlink(&source, &link).unwrap();
        let link_output = temporary.path().join("link-output");
        assert!(!run_with_flags(&link, &link_output, &[]).status.success());
        assert!(!link_output.exists());
    }
}

#[test]
fn refuses_windows_absolute_and_backslash_member_names() {
    for unsafe_name in [
        r"C:/201-p1.p1",
        r"C:\201-p1.p1",
        r"\\server\share\201-p1.p1",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir(&source).unwrap();
        write_valid_bios(&source);
        let manifest = fixture_manifest();
        let members = manifest
            .members
            .iter()
            .enumerate()
            .map(|(index, member)| {
                (
                    if index == 0 {
                        unsafe_name.to_owned()
                    } else {
                        member.name.to_owned()
                    },
                    bytes_with_crc(member.size, member.crc32),
                )
            })
            .collect::<Vec<_>>();
        write_zip(&source.join("mslug-unsafe.zip"), &members);
        let output = temporary.path().join("output");
        assert!(!run_with_flags(&source, &output, &[]).status.success());
        assert!(!output.exists());
    }
}

#[test]
fn refuses_duplicate_case_colliding_directory_and_special_entries() {
    let manifest = fixture_manifest();
    let valid_members = manifest
        .members
        .iter()
        .map(|member| {
            (
                member.name.to_owned(),
                bytes_with_crc(member.size, member.crc32),
            )
        })
        .collect::<Vec<_>>();

    for (label, special_name, mode) in [
        ("duplicate", "dup-p1.zz", Some(0o100_644)),
        ("case-collision", "201-P1.P1", Some(0o100_644)),
        ("directory", "nested/", None),
        ("symlink", "link-entry", Some(0o120_777)),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir(&source).unwrap();
        write_valid_bios(&source);
        let archive = source.join(format!("mslug-{label}.zip"));
        write_zip_with_special_entry(&archive, &valid_members, special_name, mode);
        if label == "duplicate" {
            rewrite_zip_member_name(&archive, "dup-p1.zz", "201-p1.p1");
        } else if label == "symlink" {
            mark_zip_member_as_symlink(&archive, special_name);
        }
        let output = temporary.path().join("output");
        let result = run_with_flags(&source, &output, &[]);
        assert!(
            !result.status.success(),
            "{label} unexpectedly succeeded: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(!output.exists());
    }
}

#[cfg(unix)]
#[test]
fn refuses_symlink_output_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source");
    fs::create_dir(&source).unwrap();
    write_valid_bios(&source);
    write_game(
        &source.join("mslug-complete.zip"),
        &fixture_manifest(),
        false,
        false,
    );
    let target = temporary.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("sentinel"), b"untouched").unwrap();
    let output = temporary.path().join("output-link");
    symlink(&target, &output).unwrap();

    assert!(
        !run_with_flags(&source, &output, &["--force"])
            .status
            .success()
    );
    assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched");
}

#[test]
fn output_argument_is_mandatory() {
    let temporary = tempfile::tempdir().unwrap();
    let result = run(&[temporary.path()]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--output"));
}
