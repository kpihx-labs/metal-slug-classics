use crate::audit::{AuditClass, MemberAudit, PlanOperation, audit_path, format_crc, sha256_file};
use crate::manifest::{SetManifest, manifests};
use anyhow::{Context, Result, anyhow, bail, ensure};
use crc32fast::Hasher;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write, sink};
use std::path::{Component, Path, PathBuf};
use tempfile::{Builder, TempDir};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvenanceMember {
    pub source_name: String,
    pub target_name: String,
    pub size: u64,
    pub crc32: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvenanceManifest {
    pub schema_version: u8,
    pub bios_required: bool,
    pub source_sha256: String,
    pub manifest_id: String,
    pub members: Vec<ProvenanceMember>,
    pub output_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizedArchive {
    pub archive: PathBuf,
    pub provenance: PathBuf,
    pub manifest_id: String,
    pub output_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NormalizeReport {
    pub output: PathBuf,
    pub archives: Vec<NormalizedArchive>,
    pub skipped: Vec<SkippedArchive>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SkippedArchive {
    pub archive: PathBuf,
    pub classification: AuditClass,
}

struct ReadyArchive<'a> {
    source: PathBuf,
    source_sha256: String,
    manifest: &'a SetManifest,
    target_archive: String,
    members: Vec<ProvenanceMember>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

/// Materializes verified rename-only audit plans into a new output directory.
///
/// # Errors
/// Returns an error for unsafe paths, non-ready audits, payload mismatches, I/O
/// failures, or an existing output when `force` is false.
pub fn normalize_path(
    source: &Path,
    output: &Path,
    force: bool,
    skip_blocked: bool,
) -> Result<NormalizeReport> {
    let source = validate_source(source)?;
    let output = resolve_output(&source, output)?;
    let original_output = inspect_output(&output)?;
    if original_output.is_some() && !force {
        bail!(
            "output already exists: {}; pass --force to replace it",
            output.display()
        );
    }
    let audit = audit_path(&source)?;
    ensure!(
        !audit.archives.is_empty(),
        "source contains no auditable game ZIP"
    );
    let known = manifests();
    let (ready, mut skipped) = prepare_archives(&audit.archives, &known, skip_blocked)?;
    let mut verified = Vec::with_capacity(ready.len());
    for archive in ready {
        match verify_payloads(&archive) {
            Ok(()) => verified.push(archive),
            Err(_error) if skip_blocked => skipped.push(SkippedArchive {
                archive: archive.source,
                classification: AuditClass::Corrupt,
            }),
            Err(error) => return Err(error),
        }
    }
    ensure!(
        !verified.is_empty(),
        "source contains no normalizable game ZIP"
    );
    let parent = output
        .parent()
        .ok_or_else(|| anyhow!("output must have a parent directory"))?;
    ensure!(
        parent.is_dir(),
        "output parent does not exist: {}",
        parent.display()
    );
    let stage = Builder::new()
        .prefix(".normalize-stage-")
        .tempdir_in(parent)?;
    let parent_identity = path_identity(&fs::symlink_metadata(parent)?);
    let mut normalized = Vec::with_capacity(verified.len());
    for archive in verified {
        normalized.push(materialize(&archive, stage.path())?);
    }
    sync_directory(stage.path())?;
    ensure_path_identity(
        parent,
        &parent_identity,
        "output parent changed before publication",
    )?;
    publish(&stage, &output, force, original_output.as_ref())?;
    sync_directory(&output)?;
    sync_directory(parent)?;
    for item in &mut normalized {
        let archive_name = item
            .archive
            .file_name()
            .ok_or_else(|| anyhow!("normalized archive has no filename"))?;
        let provenance_name = item
            .provenance
            .file_name()
            .ok_or_else(|| anyhow!("provenance manifest has no filename"))?;
        item.archive = output.join(archive_name);
        item.provenance = output.join(provenance_name);
    }
    Ok(NormalizeReport {
        output,
        archives: normalized,
        skipped,
    })
}

fn verify_payloads(ready: &ReadyArchive<'_>) -> Result<()> {
    let file = File::open(&ready.source)
        .with_context(|| format!("open source archive {}", ready.source.display()))?;
    let mut archive = ZipArchive::new(BufReader::new(file))
        .with_context(|| format!("parse source archive {}", ready.source.display()))?;
    for member in &ready.members {
        let mut entry = archive.by_name(&member.source_name).with_context(|| {
            format!(
                "open member {} from {}",
                member.source_name,
                ready.source.display()
            )
        })?;
        std::io::copy(&mut entry, &mut sink()).with_context(|| {
            format!(
                "verify member {} from {}",
                member.source_name,
                ready.source.display()
            )
        })?;
    }
    Ok(())
}

fn validate_source(source: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("inspect source {}", source.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "source cannot be a symlink"
    );
    ensure!(
        metadata.is_file() || metadata.is_dir(),
        "source must be a ZIP or directory"
    );
    if metadata.is_file() {
        ensure!(
            source
                .extension()
                .is_some_and(|value| value.eq_ignore_ascii_case("zip")),
            "source file must be a ZIP"
        );
    }
    source
        .canonicalize()
        .with_context(|| format!("resolve source {}", source.display()))
}

fn resolve_output(source: &Path, output: &Path) -> Result<PathBuf> {
    ensure!(!output.as_os_str().is_empty(), "output path is required");
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .with_context(|| format!("resolve output parent {}", parent.display()))?;
    let name = output
        .file_name()
        .ok_or_else(|| anyhow!("output must name a directory"))?;
    let resolved = parent.join(name);
    if source.is_dir() {
        ensure!(
            resolved != source && !resolved.starts_with(source),
            "output must be outside the source directory"
        );
    } else {
        ensure!(resolved != source, "output must differ from the source ZIP");
    }
    Ok(resolved)
}

fn prepare_archives<'a>(
    archives: &[crate::audit::ArchiveAudit],
    manifests: &'a [SetManifest],
    skip_blocked: bool,
) -> Result<(Vec<ReadyArchive<'a>>, Vec<SkippedArchive>)> {
    let mut ready = Vec::with_capacity(archives.len());
    let mut skipped = Vec::new();
    let mut targets = BTreeSet::new();
    for archive in archives {
        let metadata = fs::symlink_metadata(&archive.path)?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "source archive cannot be a symlink: {}",
            archive.path.display()
        );
        if !matches!(
            archive.classification,
            AuditClass::ModernCompatible
                | AuditClass::DocumentedLegacyDecrypted
                | AuditClass::MissingBios
        ) {
            if skip_blocked {
                skipped.push(SkippedArchive {
                    archive: archive.path.clone(),
                    classification: archive.classification,
                });
                continue;
            }
            bail!(
                "refusing {:?} archive {}",
                archive.classification,
                archive.path.display()
            );
        }
        let manifest_id = archive
            .manifest_id
            .as_deref()
            .ok_or_else(|| anyhow!("ready audit has no manifest id"))?;
        let manifest = manifests
            .iter()
            .find(|candidate| candidate.id == manifest_id)
            .ok_or_else(|| anyhow!("audit references unknown manifest {manifest_id}"))?;
        let plan = archive
            .derived_plan
            .as_ref()
            .ok_or_else(|| anyhow!("audit has no derived plan for {manifest_id}"))?;
        ensure!(!plan.blocked, "refusing blocked plan for {manifest_id}");
        ensure!(
            plan.target_archive == format!("{}.zip", manifest.driver),
            "plan target does not match manifest driver"
        );
        ensure!(
            targets.insert(plan.target_archive.clone()),
            "duplicate output archive name"
        );
        let members = plan_members(&plan.operations, &archive.members, manifest)?;
        ready.push(ReadyArchive {
            source: archive.path.clone(),
            source_sha256: archive
                .archive_sha256
                .clone()
                .ok_or_else(|| anyhow!("audit did not hash {}", archive.path.display()))?,
            manifest,
            target_archive: plan.target_archive.clone(),
            members,
        });
    }
    Ok((ready, skipped))
}

fn plan_members(
    operations: &[PlanOperation],
    audited: &[MemberAudit],
    manifest: &SetManifest,
) -> Result<Vec<ProvenanceMember>> {
    let expected = audited
        .iter()
        .map(|member| (member.expected_name.as_str(), member))
        .collect::<BTreeMap<_, _>>();
    let mut members = Vec::with_capacity(manifest.members.len());
    let mut selected_driver = false;
    let mut source_names = BTreeSet::new();
    let mut target_names = BTreeSet::new();
    for operation in operations {
        match operation {
            PlanOperation::CopyMember { source, target } => {
                ensure_safe_member_name(source)?;
                ensure_safe_member_name(target)?;
                ensure!(
                    source_names.insert(member_name_key(source)),
                    "duplicate or case-colliding plan source name: {source}"
                );
                ensure!(
                    target_names.insert(member_name_key(target)),
                    "duplicate or case-colliding plan target name: {target}"
                );
                let member = expected
                    .get(target.as_str())
                    .ok_or_else(|| anyhow!("plan target is not in audited manifest: {target}"))?;
                ensure!(
                    member.actual_name.as_deref() == Some(source),
                    "plan source changed after audit"
                );
                members.push(ProvenanceMember {
                    source_name: source.clone(),
                    target_name: target.clone(),
                    size: member.expected_size,
                    crc32: member.expected_crc32.clone(),
                });
            }
            PlanOperation::SelectDriver { driver } => {
                ensure!(driver == manifest.driver, "plan selected the wrong driver");
                selected_driver = true;
            }
        }
    }
    ensure!(selected_driver, "plan does not select a driver");
    ensure!(
        members.len() == manifest.members.len(),
        "plan does not contain every manifest member"
    );
    for (member, expected) in members.iter().zip(&manifest.members) {
        ensure!(
            member.target_name == expected.name,
            "plan member order differs from manifest"
        );
        ensure!(
            member.size == expected.size,
            "plan member size differs from manifest"
        );
        ensure!(
            member.crc32 == format_crc(expected.crc32),
            "plan member CRC differs from manifest"
        );
    }
    Ok(members)
}

fn ensure_safe_member_name(name: &str) -> Result<()> {
    let bytes = name.as_bytes();
    let windows_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    ensure!(
        !name.is_empty()
            && !name.contains('\0')
            && !name.contains('\\')
            && !name.starts_with('/')
            && !windows_drive,
        "unsafe ZIP member name: {name:?}"
    );
    ensure!(
        Path::new(name)
            .components()
            .all(|component| matches!(component, Component::Normal(_))),
        "unsafe ZIP member name: {name:?}"
    );
    Ok(())
}

fn member_name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn index_zip_members<R: Read + Seek>(
    source: &mut ZipArchive<R>,
) -> Result<BTreeMap<String, usize>> {
    let mut indices = BTreeMap::new();
    let mut portable_names = BTreeSet::new();
    for index in 0..source.len() {
        let entry = source.by_index_raw(index)?;
        let name = entry.name().to_owned();
        ensure_safe_member_name(&name)?;
        ensure_regular_zip_entry(&entry, &name)?;
        ensure!(
            indices.insert(name.clone(), index).is_none(),
            "duplicate ZIP member name: {name}"
        );
        ensure!(
            portable_names.insert(member_name_key(&name)),
            "case-colliding ZIP member name: {name}"
        );
    }
    Ok(indices)
}

fn materialize(ready: &ReadyArchive<'_>, output: &Path) -> Result<NormalizedArchive> {
    let source_file = File::open(&ready.source)
        .with_context(|| format!("open source archive {}", ready.source.display()))?;
    let opened_identity = path_identity(&source_file.metadata()?);
    ensure!(
        source_file.metadata()?.is_file(),
        "source archive is not a regular file: {}",
        ready.source.display()
    );
    ensure_open_file_is_current(&ready.source, &opened_identity)?;
    let mut source_reader = BufReader::new(source_file);
    ensure!(
        sha256_open_file(&mut source_reader)? == ready.source_sha256,
        "source archive changed after audit: {}",
        ready.source.display()
    );
    let mut source = ZipArchive::new(source_reader)?;
    let indices = index_zip_members(&mut source)?;

    let archive_path = output.join(&ready.target_archive);
    let archive_file = File::create(&archive_path)?;
    let mut writer = ZipWriter::new(BufWriter::new(archive_file));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    let mut buffer = vec![0_u8; 64 * 1024];
    for member in &ready.members {
        let index = *indices
            .get(&member.source_name)
            .ok_or_else(|| anyhow!("source member disappeared: {}", member.source_name))?;
        let mut input = source.by_index(index)?;
        writer.start_file(&member.target_name, options)?;
        let mut crc = Hasher::new();
        let mut size = 0_u64;
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            writer.write_all(&buffer[..count])?;
            crc.update(&buffer[..count]);
            size += u64::try_from(count)?;
        }
        ensure!(
            size == member.size,
            "payload size mismatch for {}",
            member.source_name
        );
        ensure!(
            format_crc(crc.finalize()) == member.crc32,
            "payload CRC mismatch for {}",
            member.source_name
        );
    }
    let mut archive_file = writer.finish()?.into_inner()?;
    archive_file.flush()?;
    archive_file.sync_all()?;
    let mut source_reader = source.into_inner();
    ensure!(
        path_identity(&source_reader.get_ref().metadata()?) == opened_identity,
        "opened source archive identity changed during normalization: {}",
        ready.source.display()
    );
    ensure!(
        sha256_open_file(&mut source_reader)? == ready.source_sha256,
        "source archive changed during normalization: {}",
        ready.source.display()
    );
    ensure_open_file_is_current(&ready.source, &opened_identity)?;
    let output_sha256 = sha256_file(&archive_path)?;
    let provenance = ProvenanceManifest {
        schema_version: 3,
        bios_required: true,
        source_sha256: ready.source_sha256.clone(),
        manifest_id: ready.manifest.id.to_owned(),
        members: ready.members.clone(),
        output_sha256: output_sha256.clone(),
    };
    let provenance_path = output.join(format!("{}.provenance.json", ready.target_archive));
    let provenance_file = File::create(&provenance_path)?;
    let mut provenance_file = BufWriter::new(provenance_file);
    serde_json::to_writer_pretty(&mut provenance_file, &provenance)?;
    provenance_file.write_all(b"\n")?;
    provenance_file.flush()?;
    provenance_file.get_ref().sync_all()?;
    Ok(NormalizedArchive {
        archive: archive_path,
        provenance: provenance_path,
        manifest_id: ready.manifest.id.to_owned(),
        output_sha256,
    })
}

fn ensure_regular_zip_entry<R: Read>(entry: &zip::read::ZipFile<'_, R>, name: &str) -> Result<()> {
    ensure!(!entry.is_dir(), "ZIP member is a directory: {name}");
    if let Some(mode) = entry.unix_mode() {
        let file_type = mode & 0o170_000;
        ensure!(
            file_type == 0 || file_type == 0o100_000,
            "ZIP member is not a regular file: {name}"
        );
    }
    Ok(())
}

fn sha256_open_file(file: &mut (impl Read + Seek)) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(format!("{:x}", digest.finalize()))
}

fn inspect_output(output: &Path) -> Result<Option<PathIdentity>> {
    match fs::symlink_metadata(output) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "output cannot be a symlink"
            );
            ensure!(metadata.is_dir(), "output must be a directory");
            Ok(Some(path_identity(&metadata)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("inspect output {}", output.display())),
    }
}

fn path_identity(metadata: &Metadata) -> PathIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

    PathIdentity {
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
        #[cfg(not(unix))]
        length: metadata.len(),
        #[cfg(not(unix))]
        modified: metadata.modified().ok(),
    }
}

fn ensure_path_identity(path: &Path, expected: &PathIdentity, message: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(!metadata.file_type().is_symlink(), "{message}");
    ensure!(path_identity(&metadata) == *expected, "{message}");
    Ok(())
}

fn ensure_open_file_is_current(path: &Path, expected: &PathIdentity) -> Result<()> {
    ensure_path_identity(
        path,
        expected,
        "source archive path changed during normalization",
    )
}

fn publish(
    stage: &TempDir,
    output: &Path,
    force: bool,
    original_output: Option<&PathIdentity>,
) -> Result<()> {
    let stage_path = stage.path().to_path_buf();
    let current_output = inspect_output(output)?;
    ensure!(
        current_output.as_ref() == original_output,
        "output changed before publication"
    );
    if original_output.is_none() {
        return fs::rename(&stage_path, output)
            .with_context(|| format!("publish output {}", output.display()));
    }
    ensure!(force, "output already exists");
    let parent = output
        .parent()
        .ok_or_else(|| anyhow!("resolved output has no parent"))?;
    let backup_guard = Builder::new()
        .prefix(".normalize-backup-")
        .tempdir_in(parent)?;
    let backup_path = backup_guard.path().to_path_buf();
    backup_guard.close()?;
    fs::rename(output, &backup_path)?;
    if let Err(error) = fs::rename(&stage_path, output) {
        let restore = fs::rename(&backup_path, output);
        return match restore {
            Ok(()) => Err(error).context("publish replacement output"),
            Err(restore_error) => Err(error).context(format!(
                "publish replacement output; rollback also failed: {restore_error}"
            )),
        };
    }
    fs::remove_dir_all(&backup_path).context("remove replaced output backup")?;
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_platform_separator_names() {
        for name in [
            "../rom.bin",
            "/rom.bin",
            "//server/share/rom.bin",
            "C:/rom.bin",
            "c:\\rom.bin",
            "dir/../rom.bin",
            "dir\\rom.bin",
            ".",
            "",
        ] {
            assert!(ensure_safe_member_name(name).is_err(), "accepted {name:?}");
        }
        assert!(ensure_safe_member_name("201-p1.p1").is_ok());
    }

    #[test]
    fn output_must_be_outside_source_location() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("source");
        fs::create_dir(&source).unwrap();
        assert!(resolve_output(&source, &source.join("normalized")).is_err());
        assert!(resolve_output(&source, &temporary.path().join("normalized")).is_ok());
    }

    #[test]
    fn forced_publish_restores_original_when_replacement_rename_fails() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("sentinel"), b"original").unwrap();
        let identity = inspect_output(&output).unwrap().unwrap();
        let stage = Builder::new()
            .prefix(".normalize-stage-")
            .tempdir_in(temporary.path())
            .unwrap();
        fs::remove_dir(stage.path()).unwrap();

        assert!(publish(&stage, &output, true, Some(&identity)).is_err());
        assert_eq!(fs::read(output.join("sentinel")).unwrap(), b"original");
    }
}
