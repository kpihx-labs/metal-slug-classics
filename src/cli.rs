use crate::audit::{ArchiveAudit, AuditReport, MemberState, audit_path};
use crate::manifest::{GameId, manifests};
use crate::netplay::{AgreementReport, compare, load as load_contract};
use crate::profile::{self, Profile, RuntimeKind};
use crate::runtime;
use anyhow::{Result, bail};
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "metal-slug-classics", version, about)]
pub struct Cli {
    #[arg(long, global = true, help = "Emit machine-readable JSON")]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Doctor,
    Audit {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    Play {
        game: GameId,
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long, help = "Prepare and verify without starting the runtime")]
        dry_run: bool,
    },
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    Lan {
        #[command(subcommand)]
        command: LanCommand,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RuntimeArgument {
    Standalone,
    Libretro,
}

impl From<RuntimeArgument> for RuntimeKind {
    fn from(value: RuntimeArgument) -> Self {
        match value {
            RuntimeArgument::Standalone => Self::Standalone,
            RuntimeArgument::Libretro => Self::Libretro,
        }
    }
}

#[derive(Debug, Subcommand)]
enum ProfileCommand {
    List,
    Create(Box<ProfileCreateArgs>),
}

#[derive(Debug, Args)]
struct ProfileCreateArgs {
    name: String,
    #[arg(long)]
    rom_dir: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "libretro")]
    runtime: RuntimeArgument,
    #[arg(long)]
    standalone_executable: Option<PathBuf>,
    #[arg(long)]
    retroarch_executable: Option<PathBuf>,
    #[arg(long)]
    libretro_core: Option<PathBuf>,
    #[arg(long)]
    runtime_compatibility_id: String,
    #[arg(long)]
    core_compatibility_id: Option<String>,
    #[arg(long)]
    runtime_sha256: String,
    #[arg(long)]
    core_sha256: Option<String>,
}

#[derive(Debug, Subcommand)]
enum LanCommand {
    Doctor,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    status: &'static str,
    platform: &'static str,
    config_root: PathBuf,
    profiles: Vec<String>,
    manifest_count: usize,
    source_audit: Option<AuditReport>,
    guarantees: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct LanDoctorReport {
    status: &'static str,
    local_contract: Option<PathBuf>,
    peer_contract: Option<PathBuf>,
    agreement: Option<AgreementReport>,
    requirements: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct CliFailure {
    code: &'static str,
    message: String,
}

impl CliFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn json(&self) -> String {
        serde_json::json!({
            "ok": false,
            "error": {
                "code": self.code,
                "message": self.message,
            }
        })
        .to_string()
    }
}

impl fmt::Display for CliFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CliFailure {}

/// Parses the CLI and executes the selected command.
///
/// # Errors
/// Returns a stable coded failure when parsing or command execution fails.
pub fn run() -> std::result::Result<(), CliFailure> {
    run_from(env::args_os())
}

fn run_from<I, T>(arguments: I) -> std::result::Result<(), CliFailure>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            error
                .print()
                .map_err(|failure| CliFailure::new("invalid-cli", failure.to_string()))?;
            return Ok(());
        }
        Err(error) => return Err(CliFailure::new("invalid-cli", error.to_string())),
    };
    match cli.command {
        Command::Doctor => {
            doctor(cli.json).map_err(|error| command_failure("doctor-failed", &error))
        }
        Command::Audit { path } => {
            audit(&path, cli.json).map_err(|error| command_failure("audit-failed", &error))
        }
        Command::Play {
            game,
            profile,
            dry_run,
        } => play(game, &profile, dry_run, cli.json)
            .map_err(|error| command_failure("play-failed", &error)),
        Command::Profile { command } => profile_command(command, cli.json)
            .map_err(|error| command_failure("profile-failed", &error)),
        Command::Lan {
            command: LanCommand::Doctor,
        } => lan_doctor(cli.json).map_err(|error| command_failure("lan-doctor-failed", &error)),
    }
}

fn command_failure(code: &'static str, error: &anyhow::Error) -> CliFailure {
    CliFailure::new(code, format!("{error:#}"))
}

#[must_use]
pub fn json_requested() -> bool {
    env::args_os().any(|argument| argument == "--json")
}

fn doctor(json: bool) -> Result<()> {
    let source_audit = env::var_os("METAL_SLUG_SOURCE")
        .map(PathBuf::from)
        .map(|path| audit_path(&path))
        .transpose()?;
    let report = DoctorReport {
        status: "ok",
        platform: env::consts::OS,
        config_root: profile::config_root()?,
        profiles: profile::list()?,
        manifest_count: manifests().len(),
        source_audit,
        guarantees: vec![
            "ZIP central-directory metadata only; no extraction",
            "no ROM or BIOS download path",
            "corrupt bytes are never described as repairable",
            "LAN launch requires exact behavioral agreement when enabled",
        ],
    };
    emit(&report, json, |value| {
        println!("metal-slug-classics doctor: {}", value.status);
        println!("platform: {}", value.platform);
        println!("config: {}", value.config_root.display());
        println!("typed manifests: {}", value.manifest_count);
        println!("profiles: {}", value.profiles.len());
        if let Some(audit) = &value.source_audit {
            println!(
                "source archives: {} (BIOS valid: {})",
                audit.archives.len(),
                audit.bios_valid
            );
        }
    })
}

fn audit(path: &std::path::Path, json: bool) -> Result<()> {
    let report = audit_path(path)?;
    emit(&report, json, print_audit)
}

fn print_audit(report: &AuditReport) {
    println!("Audit root: {}", report.root.display());
    println!(
        "BIOS: {}",
        if report.bios_valid {
            "verified"
        } else {
            "missing or invalid"
        }
    );
    if let Some(path) = &report.bios_path {
        println!("BIOS path: {}", path.display());
    }
    if let Some(error) = &report.bios_error {
        println!("BIOS error: {error}");
    }
    if report.archives.is_empty() {
        println!("No game ZIP archives found.");
    }
    for archive in &report.archives {
        print_archive(archive);
    }
}

fn print_archive(archive: &ArchiveAudit) {
    println!("\n{}: {:?}", archive.path.display(), archive.classification);
    if let Some(manifest) = &archive.manifest_id {
        println!("  manifest: {manifest}");
    }
    for member in &archive.members {
        if member.state != MemberState::Exact {
            println!(
                "  {:?}: {} (actual: {}, expected CRC {}, actual CRC {})",
                member.state,
                member.expected_name,
                member.actual_name.as_deref().unwrap_or("missing"),
                member.expected_crc32,
                member.actual_crc32.as_deref().unwrap_or("missing")
            );
        }
    }
    if let Some(plan) = &archive.derived_plan {
        println!(
            "  derived plan: {} ({})",
            plan.plan_sha256,
            if plan.blocked { "blocked" } else { "ready" }
        );
        println!("  {}", plan.note);
    }
    if let Some(error) = &archive.error {
        println!("  error: {error}");
    }
}

fn play(game: GameId, profile_name: &str, dry_run: bool, json: bool) -> Result<()> {
    let profile = profile::load(profile_name)?;
    let plan = runtime::prepare(&profile, game)?;
    if json || dry_run {
        emit(&plan, json, |value| {
            println!("runtime: {}", value.program.display());
            println!("contract: {}", value.contract_path.display());
            println!(
                "command: {} {}",
                value.program.display(),
                value.arguments.join(" ")
            );
        })?;
    }
    if dry_run {
        return Ok(());
    }
    let status = runtime::launch(&plan)?;
    if !status.success() {
        bail!("runtime exited with {status}");
    }
    Ok(())
}

fn profile_command(command: ProfileCommand, json: bool) -> Result<()> {
    match command {
        ProfileCommand::List => {
            let profiles = profile::list()?;
            emit(&profiles, json, |values| {
                if values.is_empty() {
                    println!("No profiles.");
                } else {
                    for value in values {
                        println!("{value}");
                    }
                }
            })
        }
        ProfileCommand::Create(arguments) => {
            let ProfileCreateArgs {
                name,
                rom_dir,
                runtime,
                standalone_executable,
                retroarch_executable,
                libretro_core,
                runtime_compatibility_id,
                core_compatibility_id,
                runtime_sha256,
                core_sha256,
            } = *arguments;
            let mut value =
                Profile::new(&name, rom_dir.unwrap_or(profile::default_rom_directory()?))?;
            value.runtime = runtime.into();
            value.standalone_executable = standalone_executable;
            value.retroarch_executable = retroarch_executable;
            value.libretro_core = libretro_core;
            value.runtime_compatibility_id = runtime_compatibility_id;
            value.core_compatibility_id = core_compatibility_id;
            value.expected_runtime_sha256 = Some(runtime_sha256);
            value.expected_core_sha256 = core_sha256;
            if value.runtime == RuntimeKind::Libretro
                && (value.core_compatibility_id.is_none() || value.expected_core_sha256.is_none())
            {
                bail!(
                    "--core-compatibility-id and --core-sha256 are required for libretro profiles"
                );
            }
            let path = profile::create(&value)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({ "path": path, "profile": value })
                    )?
                );
            } else {
                println!("created profile '{}' at {}", value.name, path.display());
            }
            Ok(())
        }
    }
}

fn lan_doctor(json: bool) -> Result<()> {
    let local_path = env::var_os("METAL_SLUG_LOCAL_CONTRACT").map(PathBuf::from);
    let peer_path = env::var_os("METAL_SLUG_PEER_CONTRACT").map(PathBuf::from);
    let agreement = match (&local_path, &peer_path) {
        (Some(local), Some(peer)) => Some(compare(&load_contract(local)?, &load_contract(peer)?)),
        _ => None,
    };
    let status = match &agreement {
        Some(value) if value.agreed => "ready",
        Some(_) => "blocked-mismatch",
        None => "contracts-required",
    };
    if let Some(agreement) = &agreement
        && !agreement.agreed
    {
        let fields = agreement
            .mismatches
            .iter()
            .map(|mismatch| mismatch.field.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        bail!("LAN behavioral contracts do not agree: {fields}");
    }
    let report = LanDoctorReport {
        status,
        local_contract: local_path,
        peer_contract: peer_path,
        agreement,
        requirements: vec![
            "same game and canonical typed-manifest content",
            "same runtime/core compatibility IDs, ROM, BIOS, and behavioral settings",
            "same-subnet direct RetroArch netplay with public announce disabled",
        ],
    };
    emit(&report, json, |value| {
        println!("LAN doctor: {}", value.status);
        for requirement in &value.requirements {
            println!("  - {requirement}");
        }
        if let Some(agreement) = &value.agreement {
            for mismatch in &agreement.mismatches {
                println!("  mismatch: {}", mismatch.field);
            }
        }
    })?;
    Ok(())
}

fn emit<T, F>(value: &T, json: bool, human: F) -> Result<()>
where
    T: Serialize,
    F: FnOnce(&T),
{
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        human(value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_command_failure_has_stable_envelope() {
        let failure = run_from([
            "metal-slug-classics",
            "--json",
            "audit",
            "/definitely/not/a/real/metal-slug-path",
        ])
        .unwrap_err();
        let envelope: serde_json::Value = serde_json::from_str(&failure.json()).unwrap();
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "audit-failed");
        assert!(envelope["error"]["message"].is_string());
    }

    #[test]
    fn json_parse_failure_has_stable_code() {
        let failure = run_from(["metal-slug-classics", "--json", "not-a-command"]).unwrap_err();
        let envelope: serde_json::Value = serde_json::from_str(&failure.json()).unwrap();
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "invalid-cli");
    }
}
