use crate::audit::{
    ArchiveAudit, AuditClass, AuditReport, BiosAudit, audit_bios, audit_path, sha256_file,
};
use crate::manifest::{GameId, SetManifest, manifests};
use crate::netplay::{
    SessionContract, load as load_contract, require_agreement, retroarch_netplay_arguments,
};
#[cfg(test)]
use crate::profile::NetplayRole;
use crate::profile::{Profile, RuntimeKind, config_root, portable_path};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LaunchPlan {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
    pub config_path: PathBuf,
    pub contract_path: PathBuf,
    pub session_contract: SessionContract,
    pub local_integrity: LocalRuntimeIntegrity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalRuntimeIntegrity {
    pub runtime_sha256: String,
    pub core_sha256: Option<String>,
}

#[derive(Debug)]
struct PreparedRuntime {
    program: PathBuf,
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
    working_directory: PathBuf,
    config_path: PathBuf,
    local_integrity: LocalRuntimeIntegrity,
}

/// Audits inputs, writes isolated runtime configuration, and builds a launch plan.
///
/// # Errors
/// Returns an error for unverified data, missing runtime files, configuration
/// failures, or a LAN contract mismatch.
pub fn prepare(profile: &Profile, game: GameId) -> Result<LaunchPlan> {
    let report = audit_path(&profile.rom_directory)?;
    let bios = audit_bios(&profile.bios_archive)?;
    validate_bios(&bios)?;
    let archive = select_archive(&report, profile, game)?;
    let manifest = manifest_for_archive(archive)?;
    let stem = archive
        .path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !stem.eq_ignore_ascii_case(manifest.driver) {
        bail!(
            "archive is valid but not launch-normalized: expected '{}.zip'; audit provides a deterministic plan only",
            manifest.driver
        );
    }

    let generated = config_root()?.join("generated").join(&profile.name);
    fs::create_dir_all(&generated)
        .with_context(|| format!("create generated runtime directory {}", generated.display()))?;
    validate_compatibility_ids(profile)?;
    let prepared = match profile.runtime {
        RuntimeKind::Standalone => {
            prepare_standalone(profile, manifest.driver, &bios.path, &generated)?
        }
        RuntimeKind::Libretro => prepare_libretro(profile, &archive.path, &bios.path, &generated)?,
    };
    verify_local_integrity(profile, &prepared.local_integrity)?;
    let rom_sha256 = archive
        .archive_sha256
        .clone()
        .ok_or_else(|| anyhow!("could not hash ROM archive"))?;
    let session_contract =
        build_session_contract(profile, game, &manifest, rom_sha256, bios.archive_sha256)?;
    let contract_path = generated.join("session-contract.json");
    write_json(&contract_path, &session_contract)?;

    if profile.netplay.enabled && profile.netplay.require_peer_contract {
        let peer_path = env::var_os("METAL_SLUG_PEER_CONTRACT")
            .map(PathBuf::from)
            .ok_or_else(|| {
                anyhow!(
                    "netplay requires METAL_SLUG_PEER_CONTRACT; local contract written to {}",
                    contract_path.display()
                )
            })?;
        let peer = load_contract(&peer_path)?;
        require_agreement(&session_contract, &peer)?;
    }

    Ok(LaunchPlan {
        program: prepared.program,
        arguments: prepared.arguments,
        environment: prepared.environment,
        working_directory: prepared.working_directory,
        config_path: prepared.config_path,
        contract_path,
        session_contract,
        local_integrity: prepared.local_integrity,
    })
}

fn validate_bios(bios: &BiosAudit) -> Result<()> {
    if !bios.valid {
        bail!(
            "verified Neo Geo BIOS is missing or invalid: {}",
            bios.path.display()
        );
    }
    if !bios
        .path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("neogeo.zip"))
    {
        bail!(
            "FBNeo BIOS archive must be named neogeo.zip: {}",
            bios.path.display()
        );
    }
    Ok(())
}

fn select_archive<'a>(
    report: &'a AuditReport,
    profile: &Profile,
    game: GameId,
) -> Result<&'a ArchiveAudit> {
    let archive = report
        .archives
        .iter()
        .find(|item| {
            item.manifest_id.as_deref().is_some_and(|id| {
                manifests()
                    .iter()
                    .any(|manifest| manifest.id == id && manifest.game == game)
            })
        })
        .ok_or_else(|| {
            anyhow!(
                "no recognized Metal Slug {game} archive in {}",
                profile.rom_directory.display()
            )
        })?;
    if matches!(
        archive.classification,
        AuditClass::Corrupt | AuditClass::Unknown
    ) || archive
        .derived_plan
        .as_ref()
        .is_some_and(|plan| plan.blocked)
    {
        bail!(
            "refusing launch: archive classification is {:?}",
            archive.classification
        );
    }
    Ok(archive)
}

fn manifest_for_archive(archive: &ArchiveAudit) -> Result<SetManifest> {
    let manifest_id = archive
        .manifest_id
        .as_deref()
        .ok_or_else(|| anyhow!("recognized archive has no manifest"))?;
    manifests()
        .into_iter()
        .find(|item| item.id == manifest_id)
        .ok_or_else(|| anyhow!("manifest disappeared during launch preparation"))
}

/// Executes an already verified launch plan.
///
/// # Errors
/// Returns an error when the runtime process cannot be started or waited on.
pub fn launch(plan: &LaunchPlan) -> Result<ExitStatus> {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.arguments)
        .envs(&plan.environment)
        .current_dir(&plan.working_directory);
    command
        .status()
        .with_context(|| format!("launch runtime {}", plan.program.display()))
}

fn prepare_standalone(
    profile: &Profile,
    driver: &str,
    bios_archive: &Path,
    generated: &Path,
) -> Result<PreparedRuntime> {
    prepare_standalone_for_os(profile, driver, bios_archive, generated, env::consts::OS)
}

fn prepare_standalone_for_os(
    profile: &Profile,
    driver: &str,
    bios_archive: &Path,
    generated: &Path,
    os: &str,
) -> Result<PreparedRuntime> {
    let program = required_file(
        profile.standalone_executable.as_deref(),
        "standalone_executable",
    )?;
    let data_root = generated.join("standalone-data");
    let working_directory = generated.join("standalone-work");
    fs::create_dir_all(&working_directory)?;
    let config_path = standalone_config_path(&data_root, &working_directory, os);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let rom_path = trailing_slash(&portable_path(&profile.rom_directory));
    let bios_directory = bios_archive
        .parent()
        .ok_or_else(|| anyhow!("BIOS archive has no parent directory"))?;
    let bios_path = trailing_slash(&portable_path(bios_directory));
    fs::write(
        &config_path,
        format!("szAppRomPaths[0] {rom_path}\nszAppRomPaths[1] {bios_path}\nEnableHiscores 1\n"),
    )?;
    let mut environment = BTreeMap::new();
    match os {
        "windows" => {
            environment.insert("APPDATA".to_owned(), portable_path(&data_root));
        }
        "macos" => {
            environment.insert("HOME".to_owned(), portable_path(&data_root));
        }
        _ => {
            environment.insert("XDG_DATA_HOME".to_owned(), portable_path(&data_root));
        }
    }
    let runtime_sha256 = sha256_file(&program)?;
    Ok(PreparedRuntime {
        program,
        arguments: vec!["-joy".to_owned(), "-autosave".to_owned(), driver.to_owned()],
        environment,
        config_path,
        working_directory,
        local_integrity: LocalRuntimeIntegrity {
            runtime_sha256,
            core_sha256: None,
        },
    })
}

fn prepare_libretro(
    profile: &Profile,
    archive: &Path,
    bios_archive: &Path,
    generated: &Path,
) -> Result<PreparedRuntime> {
    let program = required_file(
        profile.retroarch_executable.as_deref(),
        "retroarch_executable",
    )?;
    let core = required_file(profile.libretro_core.as_deref(), "libretro_core")?;
    let config_path = generated.join("retroarch.cfg");
    let working_directory = generated.join("retroarch-work");
    let save_directory = generated.join("saves");
    let state_directory = generated.join("states");
    fs::create_dir_all(&save_directory)?;
    fs::create_dir_all(&state_directory)?;
    fs::create_dir_all(&working_directory)?;
    let system_directory = bios_archive
        .parent()
        .ok_or_else(|| anyhow!("BIOS archive has no parent directory"))?;
    let config = format!(
        "libretro_directory = \"{}\"\nsystem_directory = \"{}\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"{}\"\nnetwork_cmd_enable = \"false\"\nnetplay_public_announce = \"false\"\nnetplay_nat_traversal = \"false\"\nnetplay_use_mitm_server = \"false\"\n",
        portable_path(core.parent().unwrap_or_else(|| Path::new("."))),
        portable_path(system_directory),
        portable_path(&save_directory),
        portable_path(&state_directory),
    );
    fs::write(&config_path, config)?;
    let mut arguments = vec![
        "--config".to_owned(),
        portable_path(&config_path),
        "-L".to_owned(),
        portable_path(&core),
    ];
    if profile.netplay.enabled {
        arguments.extend(retroarch_netplay_arguments(
            profile.netplay.role,
            profile.netplay.peer_address.as_deref(),
            profile.netplay.port,
        )?);
    }
    arguments.push(portable_path(archive));
    Ok(PreparedRuntime {
        program: program.clone(),
        arguments,
        environment: BTreeMap::new(),
        working_directory,
        config_path,
        local_integrity: LocalRuntimeIntegrity {
            runtime_sha256: sha256_file(&program)?,
            core_sha256: Some(sha256_file(&core)?),
        },
    })
}

fn validate_compatibility_ids(profile: &Profile) -> Result<()> {
    if profile.runtime_compatibility_id.trim().is_empty() {
        bail!("profile requires a non-empty runtime_compatibility_id");
    }
    if profile.runtime == RuntimeKind::Libretro
        && profile
            .core_compatibility_id
            .as_deref()
            .is_none_or(|identifier| identifier.trim().is_empty())
    {
        bail!("libretro profile requires a non-empty core_compatibility_id");
    }
    validate_expected_hash(
        "expected_runtime_sha256",
        profile.expected_runtime_sha256.as_deref(),
    )?;
    if profile.runtime == RuntimeKind::Libretro {
        validate_expected_hash(
            "expected_core_sha256",
            profile.expected_core_sha256.as_deref(),
        )?;
    }
    Ok(())
}

fn validate_expected_hash(field: &str, value: Option<&str>) -> Result<()> {
    if value
        .is_none_or(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        bail!("profile field '{field}' must be a 64-digit SHA-256 value");
    }
    Ok(())
}

fn verify_local_integrity(profile: &Profile, actual: &LocalRuntimeIntegrity) -> Result<()> {
    let expected_runtime = profile
        .expected_runtime_sha256
        .as_deref()
        .ok_or_else(|| anyhow!("profile field 'expected_runtime_sha256' is required"))?;
    if !expected_runtime.eq_ignore_ascii_case(&actual.runtime_sha256) {
        bail!("local runtime SHA-256 does not match the profile pin");
    }
    if profile.runtime == RuntimeKind::Libretro {
        let expected_core = profile
            .expected_core_sha256
            .as_deref()
            .ok_or_else(|| anyhow!("profile field 'expected_core_sha256' is required"))?;
        let actual_core = actual
            .core_sha256
            .as_deref()
            .ok_or_else(|| anyhow!("libretro core hash was not computed"))?;
        if !expected_core.eq_ignore_ascii_case(actual_core) {
            bail!("local libretro core SHA-256 does not match the profile pin");
        }
    }
    Ok(())
}

fn build_session_contract(
    profile: &Profile,
    game: GameId,
    manifest: &SetManifest,
    rom_sha256: String,
    bios_sha256: String,
) -> Result<SessionContract> {
    validate_compatibility_ids(profile)?;
    Ok(SessionContract {
        protocol_version: 2,
        game: game.to_string(),
        manifest_id: manifest.id.to_owned(),
        manifest_content_sha256: manifest.content_sha256()?,
        runtime_compatibility_id: profile.runtime_compatibility_id.clone(),
        core_compatibility_id: match profile.runtime {
            RuntimeKind::Standalone => None,
            RuntimeKind::Libretro => profile.core_compatibility_id.clone(),
        },
        rom_sha256,
        bios_sha256,
        behavior_sha256: profile.behavior_sha256()?,
    })
}

fn required_file(path: Option<&Path>, field: &str) -> Result<PathBuf> {
    let path = path.ok_or_else(|| anyhow!("profile field '{field}' is required"))?;
    if !path.is_file() {
        bail!("profile field '{field}' is not a file: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("resolve profile field '{field}' at {}", path.display()))
}

fn standalone_config_path(data_root: &Path, working_directory: &Path, os: &str) -> PathBuf {
    match os {
        "windows" => working_directory.join("fbneo.ini"),
        "macos" => data_root.join("Library/Application Support/fbneo/config/fbneo.ini"),
        _ => data_root.join("fbneo/config/fbneo.ini"),
    }
}

fn trailing_slash(path: &str) -> String {
    if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::netplay::compare;
    use tempfile::tempdir;

    fn file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn libretro_profile(name: &str, root: &Path, binary_tag: &str) -> Profile {
        let mut profile = Profile::new(name, root.join("roms")).unwrap();
        let runtime = root.join(format!("runtime-{binary_tag}"));
        let core = root.join(format!("core-{binary_tag}"));
        file(&runtime, format!("runtime-{binary_tag}").as_bytes());
        file(&core, format!("core-{binary_tag}").as_bytes());
        profile.retroarch_executable = Some(runtime);
        profile.libretro_core = Some(core);
        profile.expected_runtime_sha256 =
            Some(sha256_file(profile.retroarch_executable.as_deref().unwrap()).unwrap());
        profile.expected_core_sha256 =
            Some(sha256_file(profile.libretro_core.as_deref().unwrap()).unwrap());
        profile.runtime_compatibility_id = "retroarch-cross-platform-netplay-v1".to_owned();
        profile.core_compatibility_id = Some("fbneo-custom-driver-abi-v1".to_owned());
        profile.netplay.enabled = true;
        profile
    }

    #[test]
    fn standalone_rom_path_always_has_separator() {
        assert_eq!(trailing_slash("roms"), "roms/");
        assert_eq!(trailing_slash("roms/"), "roms/");
    }

    #[test]
    fn retroarch_command_binds_exact_bios_system_directory() {
        let directory = tempdir().unwrap();
        let mut profile = libretro_profile("client", directory.path(), "linux");
        profile.netplay.role = NetplayRole::Client;
        profile.netplay.peer_address = Some("192.0.2.20".to_owned());
        profile.netplay.port = 55_440;
        let archive = directory.path().join("roms/mslug3.zip");
        let bios = directory.path().join("verified-bios/neogeo.zip");
        file(&archive, b"synthetic archive placeholder");
        file(&bios, b"synthetic bios placeholder");

        let prepared = prepare_libretro(
            &profile,
            &archive,
            &bios,
            &directory.path().join("generated"),
        )
        .unwrap();
        let config = fs::read_to_string(&prepared.config_path).unwrap();
        assert_eq!(
            config,
            format!(
                "libretro_directory = \"{}\"\nsystem_directory = \"{}\"\nsavefile_directory = \"{}\"\nsavestate_directory = \"{}\"\nnetwork_cmd_enable = \"false\"\nnetplay_public_announce = \"false\"\nnetplay_nat_traversal = \"false\"\nnetplay_use_mitm_server = \"false\"\n",
                portable_path(
                    profile
                        .libretro_core
                        .as_deref()
                        .unwrap()
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                ),
                portable_path(bios.parent().unwrap()),
                portable_path(&directory.path().join("generated/saves")),
                portable_path(&directory.path().join("generated/states")),
            )
        );
        assert_eq!(
            prepared.arguments,
            [
                "--config".to_owned(),
                portable_path(&prepared.config_path),
                "-L".to_owned(),
                portable_path(profile.libretro_core.as_deref().unwrap()),
                "--connect".to_owned(),
                "192.0.2.20".to_owned(),
                "--port".to_owned(),
                "55440".to_owned(),
                portable_path(&archive),
            ]
        );
        assert!(prepared.local_integrity.core_sha256.is_some());
    }

    #[test]
    fn windows_standalone_uses_isolated_working_directory_config() {
        let directory = tempdir().unwrap();
        let mut profile = Profile::new("windows", directory.path().join("roms")).unwrap();
        profile.runtime = RuntimeKind::Standalone;
        profile.runtime_compatibility_id = "fbneo-standalone-driver-abi-v1".to_owned();
        let executable = directory.path().join("fbneo.exe");
        let bios = directory.path().join("bios/neogeo.zip");
        file(&executable, b"synthetic windows executable");
        file(&bios, b"synthetic bios placeholder");
        profile.standalone_executable = Some(executable);
        profile.expected_runtime_sha256 =
            Some(sha256_file(profile.standalone_executable.as_deref().unwrap()).unwrap());
        let generated = directory.path().join("generated");

        let prepared =
            prepare_standalone_for_os(&profile, "mslug", &bios, &generated, "windows").unwrap();
        assert_eq!(
            prepared.working_directory,
            generated.join("standalone-work")
        );
        assert_eq!(
            prepared.config_path,
            prepared.working_directory.join("fbneo.ini")
        );
        assert_eq!(
            prepared.environment.get("APPDATA"),
            Some(&portable_path(&generated.join("standalone-data")))
        );
        let config = fs::read_to_string(&prepared.config_path).unwrap();
        assert!(config.contains("szAppRomPaths[0]"));
        assert!(config.contains("szAppRomPaths[1]"));
        verify_local_integrity(&profile, &prepared.local_integrity).unwrap();
        profile.expected_runtime_sha256 = Some("0".repeat(64));
        assert!(verify_local_integrity(&profile, &prepared.local_integrity).is_err());
    }

    #[test]
    fn macos_standalone_uses_application_support_config_root() {
        let directory = tempdir().unwrap();
        let mut profile = Profile::new("macos", directory.path().join("roms")).unwrap();
        profile.runtime = RuntimeKind::Standalone;
        profile.runtime_compatibility_id = "fbneo-standalone-driver-abi-v1".to_owned();
        let executable = directory.path().join("fbneo");
        let bios = directory.path().join("bios/neogeo.zip");
        file(&executable, b"synthetic macos executable");
        file(&bios, b"synthetic bios placeholder");
        profile.standalone_executable = Some(executable);
        profile.expected_runtime_sha256 =
            Some(sha256_file(profile.standalone_executable.as_deref().unwrap()).unwrap());
        let generated = directory.path().join("generated");

        let prepared =
            prepare_standalone_for_os(&profile, "mslug", &bios, &generated, "macos").unwrap();
        assert_eq!(
            prepared.config_path,
            generated.join("standalone-data/Library/Application Support/fbneo/config/fbneo.ini")
        );
        assert_eq!(
            prepared.environment.get("HOME"),
            Some(&portable_path(&generated.join("standalone-data")))
        );
        verify_local_integrity(&profile, &prepared.local_integrity).unwrap();
    }

    #[test]
    fn host_and_client_with_different_paths_and_binaries_agree_behaviorally() {
        let host_root = tempdir().unwrap();
        let client_root = tempdir().unwrap();
        let mut host = libretro_profile("host", host_root.path(), "linux");
        host.netplay.role = NetplayRole::Host;
        host.netplay.port = 55_435;
        let mut client = libretro_profile("client", client_root.path(), "windows");
        client.netplay.role = NetplayRole::Client;
        client.netplay.peer_address = Some("192.0.2.30".to_owned());
        client.netplay.port = 60_001;
        let host_bios = host_root.path().join("bios/neogeo.zip");
        let client_bios = client_root.path().join("other-bios/neogeo.zip");
        file(&host_bios, b"bios");
        file(&client_bios, b"bios");
        let host_archive = host_root.path().join("roms/mslug3.zip");
        let client_archive = client_root.path().join("collection/mslug3.zip");
        file(&host_archive, b"rom");
        file(&client_archive, b"rom");
        let manifest = manifests()
            .into_iter()
            .find(|candidate| candidate.id == "mslug3-modern")
            .unwrap();

        let host_contract = build_session_contract(
            &host,
            GameId::Three,
            &manifest,
            "same-rom".to_owned(),
            "same-bios".to_owned(),
        )
        .unwrap();
        let client_contract = build_session_contract(
            &client,
            GameId::Three,
            &manifest,
            "same-rom".to_owned(),
            "same-bios".to_owned(),
        )
        .unwrap();
        assert!(compare(&host_contract, &client_contract).agreed);

        let host_runtime = prepare_libretro(
            &host,
            &host_archive,
            &host_bios,
            &host_root.path().join("generated"),
        )
        .unwrap();
        let client_runtime = prepare_libretro(
            &client,
            &client_archive,
            &client_bios,
            &client_root.path().join("generated"),
        )
        .unwrap();
        assert_eq!(
            host_runtime.arguments,
            [
                "--config".to_owned(),
                portable_path(&host_runtime.config_path),
                "-L".to_owned(),
                portable_path(host.libretro_core.as_deref().unwrap()),
                "--host".to_owned(),
                "--port".to_owned(),
                "55435".to_owned(),
                portable_path(&host_archive),
            ]
        );
        assert_eq!(
            client_runtime.arguments,
            [
                "--config".to_owned(),
                portable_path(&client_runtime.config_path),
                "-L".to_owned(),
                portable_path(client.libretro_core.as_deref().unwrap()),
                "--connect".to_owned(),
                "192.0.2.30".to_owned(),
                "--port".to_owned(),
                "60001".to_owned(),
                portable_path(&client_archive),
            ]
        );
        assert_ne!(
            host_runtime.local_integrity.runtime_sha256,
            client_runtime.local_integrity.runtime_sha256
        );
        assert_ne!(
            host_runtime.local_integrity.core_sha256,
            client_runtime.local_integrity.core_sha256
        );
        verify_local_integrity(&host, &host_runtime.local_integrity).unwrap();
        verify_local_integrity(&client, &client_runtime.local_integrity).unwrap();
    }
}
