use crate::manifest::{LayoutKind, SetManifest, manifests};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use zip::ZipArchive;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditClass {
    ModernCompatible,
    DocumentedLegacyDecrypted,
    Corrupt,
    MissingBios,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberState {
    Exact,
    RenameRequired,
    Missing,
    Corrupt,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ZipMember {
    pub name: String,
    pub size: u64,
    pub crc32: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberAudit {
    pub expected_name: String,
    pub actual_name: Option<String>,
    pub expected_size: u64,
    pub actual_size: Option<u64>,
    pub expected_crc32: String,
    pub actual_crc32: Option<String>,
    pub state: MemberState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
pub enum PlanOperation {
    CopyMember { source: String, target: String },
    SelectDriver { driver: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DerivedManifestPlan {
    pub target_archive: String,
    pub operations: Vec<PlanOperation>,
    pub plan_sha256: String,
    pub blocked: bool,
    pub note: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ArchiveAudit {
    pub path: PathBuf,
    pub archive_sha256: Option<String>,
    pub classification: AuditClass,
    pub manifest_id: Option<String>,
    pub identified_layout: Option<LayoutKind>,
    pub members: Vec<MemberAudit>,
    pub extra_members: Vec<ZipMember>,
    pub derived_plan: Option<DerivedManifestPlan>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AuditReport {
    pub root: PathBuf,
    pub bios_path: Option<PathBuf>,
    pub bios_valid: bool,
    pub bios_sha256: Option<String>,
    pub bios_error: Option<String>,
    pub archives: Vec<ArchiveAudit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BiosAudit {
    pub path: PathBuf,
    pub archive_sha256: String,
    pub valid: bool,
    pub available_neogeo_modes: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug)]
struct Candidate<'a> {
    manifest: &'a SetManifest,
    members: Vec<MemberAudit>,
    matched: usize,
    filename_hits: usize,
    used: BTreeSet<usize>,
}

#[must_use]
pub fn format_crc(crc32: u32) -> String {
    format!("{crc32:08x}")
}

/// Reads ZIP central-directory metadata without extracting member payloads.
///
/// # Errors
/// Returns an error when the file cannot be opened or is not a readable ZIP.
pub fn inspect_zip(path: &Path) -> Result<Vec<ZipMember>> {
    let file = File::open(path).with_context(|| format!("open ZIP {}", path.display()))?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("read ZIP central directory {}", path.display()))?;
    let mut members = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .with_context(|| format!("inspect ZIP member #{index}"))?;
        if entry.is_dir() {
            continue;
        }
        members.push(ZipMember {
            name: entry.name().to_owned(),
            size: entry.size(),
            crc32: entry.crc32(),
        });
    }
    members.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    Ok(members)
}

/// Computes a file's SHA-256 digest.
///
/// # Errors
/// Returns an error when the file cannot be opened or read.
pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("open {} for hashing", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// Audits the exact BIOS archive path used by a runtime.
///
/// # Errors
/// Returns an error when the path is missing, cannot be canonicalized, or
/// cannot be hashed. Invalid ZIP/BIOS contents are returned as `valid: false`.
pub fn audit_bios(path: &Path) -> Result<BiosAudit> {
    let exact_path = path
        .canonicalize()
        .with_context(|| format!("resolve BIOS archive {}", path.display()))?;
    let archive_sha256 = sha256_file(&exact_path)?;
    match inspect_zip(&exact_path) {
        Ok(members) => {
            let available_neogeo_modes = available_neogeo_modes(&members)
                .into_iter()
                .map(str::to_owned)
                .collect();
            Ok(BiosAudit {
                path: exact_path,
                archive_sha256,
                valid: valid_bios(&members),
                available_neogeo_modes,
                error: None,
            })
        }
        Err(error) => Ok(BiosAudit {
            path: exact_path,
            archive_sha256,
            valid: false,
            available_neogeo_modes: Vec::new(),
            error: Some(error.to_string()),
        }),
    }
}

/// Audits one ZIP or all ZIP files directly inside a directory.
///
/// # Errors
/// Returns an error for a missing path or an unreadable directory.
pub fn audit_path(path: &Path) -> Result<AuditReport> {
    if !path.exists() {
        return Err(anyhow!("audit path does not exist: {}", path.display()));
    }

    let (root, archives) = if path.is_dir() {
        let mut archives = fs::read_dir(path)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|candidate| {
                candidate
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
                    && candidate
                        .file_name()
                        .is_some_and(|name| is_collection_archive(&name.to_string_lossy()))
            })
            .collect::<Vec<_>>();
        archives.sort();
        (path.to_path_buf(), archives)
    } else {
        let root = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        (root, vec![path.to_path_buf()])
    };

    let bios_path = archives
        .iter()
        .find(|candidate| {
            candidate
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("neogeo.zip"))
        })
        .cloned()
        .or_else(|| {
            let sibling = root.join("neogeo.zip");
            sibling.is_file().then_some(sibling)
        })
        .or_else(|| {
            let sibling = root.parent()?.join("neogeo.zip");
            sibling.is_file().then_some(sibling)
        });
    let bios_audit = bios_path.as_deref().map(audit_bios).transpose()?;
    let bios_valid = bios_audit.as_ref().is_some_and(|bios| bios.valid);

    let mut results = Vec::new();
    for archive in archives {
        if archive
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("neogeo.zip"))
        {
            continue;
        }
        results.push(audit_archive(&archive, bios_valid));
    }

    Ok(AuditReport {
        root,
        bios_path: bios_audit.as_ref().map(|bios| bios.path.clone()),
        bios_valid,
        bios_sha256: bios_audit.as_ref().map(|bios| bios.archive_sha256.clone()),
        bios_error: bios_audit.and_then(|bios| bios.error),
        archives: results,
    })
}

fn is_collection_archive(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "neogeo.zip"
        || name
            .strip_suffix(".zip")
            .is_some_and(|stem| stem.starts_with("metalslug") || stem.starts_with("mslug"))
}

fn audit_archive(path: &Path, bios_valid: bool) -> ArchiveAudit {
    let archive_sha256 = sha256_file(path).ok();
    let observed = match inspect_zip(path) {
        Ok(members) => members,
        Err(error) => {
            return ArchiveAudit {
                path: path.to_path_buf(),
                archive_sha256,
                classification: AuditClass::Corrupt,
                manifest_id: None,
                identified_layout: None,
                members: Vec::new(),
                extra_members: Vec::new(),
                derived_plan: None,
                error: Some(error.to_string()),
            };
        }
    };

    let known = manifests();
    let candidate = known
        .iter()
        .map(|manifest| compare(manifest, &observed))
        .max_by_key(|item| {
            (
                item.matched,
                item.filename_hits,
                usize::MAX - item.manifest.members.len(),
            )
        });
    let Some(candidate) = candidate else {
        return unknown_archive(path, archive_sha256, observed);
    };
    let threshold = candidate.manifest.members.len().div_ceil(3).max(2);
    if candidate.matched < threshold && candidate.filename_hits < 2 {
        return unknown_archive(path, archive_sha256, observed);
    }

    let damaged = candidate
        .members
        .iter()
        .any(|member| matches!(member.state, MemberState::Missing | MemberState::Corrupt));
    let classification = if damaged {
        AuditClass::Corrupt
    } else if !bios_valid {
        AuditClass::MissingBios
    } else {
        match candidate.manifest.layout {
            LayoutKind::ModernCompatible => AuditClass::ModernCompatible,
            LayoutKind::DocumentedLegacyDecrypted => AuditClass::DocumentedLegacyDecrypted,
        }
    };
    let extra_members = observed
        .iter()
        .enumerate()
        .filter(|(index, _)| !candidate.used.contains(index))
        .map(|(_, member)| member.clone())
        .collect();
    let derived_plan = Some(build_plan(&candidate, damaged));

    ArchiveAudit {
        path: path.to_path_buf(),
        archive_sha256,
        classification,
        manifest_id: Some(candidate.manifest.id.to_owned()),
        identified_layout: Some(candidate.manifest.layout),
        members: candidate.members,
        extra_members,
        derived_plan,
        error: None,
    }
}

fn compare<'a>(manifest: &'a SetManifest, observed: &[ZipMember]) -> Candidate<'a> {
    let mut used = BTreeSet::new();
    let mut members = Vec::with_capacity(manifest.members.len());
    let mut matched = 0;
    let mut filename_hits = 0;

    for expected in &manifest.members {
        let exact_name = observed.iter().enumerate().find(|(index, actual)| {
            !used.contains(index) && basename(&actual.name).eq_ignore_ascii_case(expected.name)
        });
        let found = exact_name.or_else(|| {
            observed.iter().enumerate().find(|(index, actual)| {
                !used.contains(index)
                    && actual.size == expected.size
                    && actual.crc32 == expected.crc32
            })
        });
        let (actual_name, actual_size, actual_crc32, state) = match found {
            Some((index, actual)) => {
                used.insert(index);
                let name_matches = basename(&actual.name).eq_ignore_ascii_case(expected.name);
                if name_matches {
                    filename_hits += 1;
                }
                if actual.size == expected.size && actual.crc32 == expected.crc32 {
                    matched += 1;
                    (
                        Some(actual.name.clone()),
                        Some(actual.size),
                        Some(format_crc(actual.crc32)),
                        if name_matches {
                            MemberState::Exact
                        } else {
                            MemberState::RenameRequired
                        },
                    )
                } else {
                    (
                        Some(actual.name.clone()),
                        Some(actual.size),
                        Some(format_crc(actual.crc32)),
                        MemberState::Corrupt,
                    )
                }
            }
            None => (None, None, None, MemberState::Missing),
        };
        members.push(MemberAudit {
            expected_name: expected.name.to_owned(),
            actual_name,
            expected_size: expected.size,
            actual_size,
            expected_crc32: format_crc(expected.crc32),
            actual_crc32,
            state,
        });
    }
    Candidate {
        manifest,
        members,
        matched,
        filename_hits,
        used,
    }
}

fn build_plan(candidate: &Candidate<'_>, damaged: bool) -> DerivedManifestPlan {
    let mut operations = Vec::new();
    if !damaged {
        for member in &candidate.members {
            if let Some(source) = &member.actual_name {
                operations.push(PlanOperation::CopyMember {
                    source: source.clone(),
                    target: member.expected_name.clone(),
                });
            }
        }
        operations.push(PlanOperation::SelectDriver {
            driver: candidate.manifest.driver.to_owned(),
        });
    }
    let target_archive = format!("{}.zip", candidate.manifest.driver);
    let canonical =
        serde_json::to_vec(&(candidate.manifest.id, &target_archive, &operations, damaged))
            .expect("derived plan is serializable");
    let plan_sha256 = format!("{:x}", Sha256::digest(canonical));
    DerivedManifestPlan {
        target_archive,
        operations,
        plan_sha256,
        blocked: damaged,
        note: if damaged {
            "blocked: missing or corrupt bytes require a lawful valid source; no repair is possible from this archive"
        } else {
            "deterministic plan only; no ROM bytes were extracted, transformed, or written"
        }
        .to_owned(),
    }
}

fn unknown_archive(
    path: &Path,
    archive_sha256: Option<String>,
    observed: Vec<ZipMember>,
) -> ArchiveAudit {
    ArchiveAudit {
        path: path.to_path_buf(),
        archive_sha256,
        classification: AuditClass::Unknown,
        manifest_id: None,
        identified_layout: None,
        members: Vec::new(),
        extra_members: observed,
        derived_plan: None,
        error: None,
    }
}

fn basename(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn valid_bios(members: &[ZipMember]) -> bool {
    let index = members
        .iter()
        .map(|member| {
            (
                (member.size, member.crc32),
                basename(&member.name).to_ascii_lowercase(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let fixed = [
        (0x02_0000, 0x9441_6d67),
        (0x02_0000, 0xc2ea_0cfd),
        (0x02_0000, 0x5a86_cff2),
    ];
    let system = [
        0x91b6_4be3,
        0x9036_d879,
        0xc7f2_fa45,
        0xe729_43de,
        0x62f0_21f4,
        0x2723_a5b5,
        0x1179_a30f,
        0x2025_b7a2,
        0xf0e8_f27d,
        0xaced_e59c,
        0x9fb0_abe4,
        0x03cc_9f6a,
        0x486c_b450,
        0xdff6_d41f,
        0xfbc6_d469,
    ];
    fixed.iter().all(|signature| index.contains_key(signature))
        && members.iter().any(|member| {
            matches!(member.size, 0x02_0000 | 0x08_0000) && system.contains(&member.crc32)
        })
}

fn available_neogeo_modes(members: &[ZipMember]) -> Vec<&'static str> {
    const MODES: &[(&str, &[(u64, u32)])] = &[
        (
            "MVS_EUR",
            &[
                (0x02_0000, 0x91b6_4be3),
                (0x02_0000, 0x9036_d879),
                (0x02_0000, 0xc7f2_fa45),
                (0x08_0000, 0x03cc_9f6a),
            ],
        ),
        (
            "MVS_USA",
            &[
                (0x02_0000, 0xe729_43de),
                (0x02_0000, 0x62f0_21f4),
                (0x02_0000, 0x2723_a5b5),
                (0x02_0000, 0x1179_a30f),
                (0x02_0000, 0x2025_b7a2),
            ],
        ),
        (
            "MVS_JAP",
            &[
                (0x02_0000, 0xf0e8_f27d),
                (0x02_0000, 0xaced_e59c),
                (0x02_0000, 0x9fb0_abe4),
                (0x08_0000, 0x486c_b450),
                (0x02_0000, 0xdff6_d41f),
                (0x02_0000, 0xfbc6_d469),
            ],
        ),
        ("AES_EUR", &[(0x02_0000, 0xd27a_71f1)]),
        ("AES_JAP", &[(0x02_0000, 0x16d0_c132)]),
    ];
    MODES
        .iter()
        .filter_map(|(mode, signatures)| {
            members
                .iter()
                .any(|member| signatures.contains(&(member.size, member.crc32)))
                .then_some(*mode)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    fn write_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for (name, bytes) in files {
            archive
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn inspects_zip_metadata_without_extracting() {
        let directory = tempdir().unwrap();
        let archive = directory.path().join("fixture.zip");
        write_zip(&archive, &[("nested/example.bin", b"known metadata")]);
        let members = inspect_zip(&archive).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].name, "nested/example.bin");
        assert_eq!(members[0].size, 14);
        assert_eq!(members[0].crc32, 0x0c7d_f2f9);
        assert!(!directory.path().join("nested").exists());
    }

    #[test]
    fn recognizes_crc_match_under_legacy_filename_as_rename() {
        let manifest = SetManifest {
            id: "fixture",
            game: crate::manifest::GameId::One,
            driver: "fixture",
            layout: LayoutKind::DocumentedLegacyDecrypted,
            members: vec![crate::manifest::RomMember {
                name: "canonical.bin",
                size: 4,
                crc32: 0xdb17_20a5,
            }],
        };
        let observed = vec![ZipMember {
            name: "old.rom".to_owned(),
            size: 4,
            crc32: 0xdb17_20a5,
        }];
        let candidate = compare(&manifest, &observed);
        assert_eq!(candidate.members[0].state, MemberState::RenameRequired);
    }

    #[test]
    fn corrupt_plan_is_blocked_and_never_claims_repair() {
        let known = manifests();
        let manifest = known
            .iter()
            .find(|item| item.id == "mslugx-modern")
            .unwrap();
        let observed = manifest
            .members
            .iter()
            .map(|member| ZipMember {
                name: member.name.to_owned(),
                size: member.size,
                crc32: if member.name == "250-c3.c3" {
                    0xa9e1_a296
                } else {
                    member.crc32
                },
            })
            .collect::<Vec<_>>();
        let candidate = compare(manifest, &observed);
        let plan = build_plan(&candidate, true);
        assert!(plan.blocked);
        assert_eq!(plan.operations, Vec::new());
        assert!(plan.note.contains("no repair is possible"));
    }

    #[test]
    fn known_manifest_metadata_produces_a_deterministic_ready_plan() {
        let known = manifests();
        let manifest = known
            .iter()
            .find(|item| item.id == "mslug3-neoragex-decrypted")
            .unwrap();
        let observed = manifest
            .members
            .iter()
            .map(|member| ZipMember {
                name: member.name.replace("256-", "ms3_"),
                size: member.size,
                crc32: member.crc32,
            })
            .collect::<Vec<_>>();
        let first = compare(manifest, &observed);
        let second = compare(manifest, &observed);
        assert_eq!(first.matched, manifest.members.len());
        assert!(
            first
                .members
                .iter()
                .all(|member| member.state == MemberState::RenameRequired)
        );
        let first_plan = build_plan(&first, false);
        let second_plan = build_plan(&second, false);
        assert!(!first_plan.blocked);
        assert_eq!(first_plan, second_plan);
        assert_eq!(first_plan.operations.len(), manifest.members.len() + 1);
    }

    #[test]
    fn bios_requires_system_and_all_three_common_components() {
        let mut members = vec![
            ZipMember {
                name: "sp-s2.sp1".to_owned(),
                size: 0x02_0000,
                crc32: 0x9036_d879,
            },
            ZipMember {
                name: "sm1.sm1".to_owned(),
                size: 0x02_0000,
                crc32: 0x9441_6d67,
            },
            ZipMember {
                name: "sfix.sfix".to_owned(),
                size: 0x02_0000,
                crc32: 0xc2ea_0cfd,
            },
            ZipMember {
                name: "000-lo.lo".to_owned(),
                size: 0x02_0000,
                crc32: 0x5a86_cff2,
            },
        ];
        assert!(valid_bios(&members));
        assert_eq!(available_neogeo_modes(&members), ["MVS_EUR"]);
        members.pop();
        assert!(!valid_bios(&members));
    }

    #[test]
    fn malformed_zip_is_reported_as_corrupt() {
        let directory = tempdir().unwrap();
        let archive = directory.path().join("mslugx.zip");
        fs::write(&archive, b"not a zip").unwrap();
        let report = audit_path(&archive).unwrap();
        assert_eq!(report.archives.len(), 1);
        assert_eq!(report.archives[0].classification, AuditClass::Corrupt);
        assert!(report.archives[0].error.is_some());
    }

    #[test]
    fn directory_audit_ignores_unrelated_neo_geo_archives() {
        let directory = tempdir().unwrap();
        write_zip(
            &directory.path().join("metalslug1.zip"),
            &[("one.bin", b"one")],
        );
        write_zip(&directory.path().join("mslug2.zip"), &[("two.bin", b"two")]);
        write_zip(
            &directory.path().join("kof98.zip"),
            &[("other.bin", b"other")],
        );

        let report = audit_path(directory.path()).unwrap();
        let names = report
            .archives
            .iter()
            .filter_map(|archive| archive.path.file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, ["metalslug1.zip", "mslug2.zip"]);
    }

    #[test]
    fn directory_audit_finds_bios_next_to_roms_directory() {
        let directory = tempdir().unwrap();
        let roms = directory.path().join("ROMs");
        fs::create_dir(&roms).unwrap();
        write_zip(
            &directory.path().join("neogeo.zip"),
            &[("placeholder", b"bios")],
        );
        write_zip(&roms.join("metalslug1.zip"), &[("one.bin", b"one")]);

        let report = audit_path(&roms).unwrap();
        assert_eq!(
            report.bios_path.as_deref(),
            Some(
                directory
                    .path()
                    .join("neogeo.zip")
                    .canonicalize()
                    .unwrap()
                    .as_path()
            )
        );
    }
}
