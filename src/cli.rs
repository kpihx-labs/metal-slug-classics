use crate::audit::{ArchiveAudit, AuditReport, MemberState, audit_path};
use crate::manifest::{GameId, manifests};
use crate::netplay::{
    AgreementReport, DEFAULT_DISCOVERY_PORT, DEFAULT_GAME_PORT, DEFAULT_MULTICAST_ADDRESS,
    DiscoveryOptions, DiscoveryPacket, advertise_once, compare, discover, load as load_contract,
    local_interfaces, new_packet, now_unix_ms, select_compatible,
};
use crate::profile::{self, NetplayRole, Profile, RuntimeKind};
use crate::runtime;
use anyhow::{Result, bail};
use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

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
    Doctor(DiscoveryArgs),
    Discover(DiscoverArgs),
    Host(LanHostArgs),
    Join(LanJoinArgs),
}

#[derive(Clone, Debug, Args)]
struct DiscoveryArgs {
    #[arg(
        long = "interface",
        help = "Local broadcast-capable IPv4 address to use; repeatable; loopback allowed for diagnostics"
    )]
    interfaces: Vec<Ipv4Addr>,
    #[arg(long, default_value_t = DEFAULT_DISCOVERY_PORT)]
    discovery_port: u16,
    #[arg(long, default_value_t = DEFAULT_MULTICAST_ADDRESS)]
    multicast_address: Ipv4Addr,
    #[arg(
        long,
        help = "Permit use of a broadcast-capable public IPv4 address selected explicitly or by the active default route"
    )]
    allow_public_lan: bool,
}

impl DiscoveryArgs {
    fn options(&self, timeout: Duration) -> DiscoveryOptions {
        DiscoveryOptions {
            interfaces: self.interfaces.clone(),
            discovery_port: self.discovery_port,
            multicast_address: self.multicast_address,
            timeout,
            allow_public: self.allow_public_lan,
        }
    }
}

#[derive(Debug, Args)]
struct DiscoverArgs {
    #[command(flatten)]
    discovery: DiscoveryArgs,
    #[arg(long, default_value_t = 3_000)]
    timeout_ms: u64,
}

#[derive(Debug, Args)]
struct LanHostArgs {
    game: GameId,
    #[arg(long, default_value = "default")]
    profile: String,
    #[command(flatten)]
    discovery: DiscoveryArgs,
    #[arg(long, default_value_t = DEFAULT_GAME_PORT)]
    game_port: u16,
    #[arg(long, default_value_t = 2_000)]
    advertisement_ttl_ms: u64,
    #[arg(long, default_value_t = 120)]
    reconnect_seconds: u64,
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct LanJoinArgs {
    game: GameId,
    #[arg(long, default_value = "default")]
    profile: String,
    #[command(flatten)]
    discovery: DiscoveryArgs,
    #[arg(long, default_value_t = 3_000)]
    timeout_ms: u64,
    #[arg(long, help = "Reconnect to this advertised session ID")]
    session_id: Option<String>,
    #[arg(long)]
    dry_run: bool,
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
    interfaces: Vec<Ipv4Addr>,
    discovery_port: u16,
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
        Command::Lan { command } => match command {
            LanCommand::Doctor(arguments) => lan_doctor(&arguments, cli.json)
                .map_err(|error| command_failure("lan-doctor-failed", &error)),
            LanCommand::Discover(arguments) => lan_discover(&arguments, cli.json)
                .map_err(|error| command_failure("lan-discover-failed", &error)),
            LanCommand::Host(arguments) => lan_host(&arguments, cli.json)
                .map_err(|error| command_failure("lan-host-failed", &error)),
            LanCommand::Join(arguments) => lan_join(&arguments, cli.json)
                .map_err(|error| command_failure("lan-join-failed", &error)),
        },
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

fn lan_doctor(arguments: &DiscoveryArgs, json: bool) -> Result<()> {
    let interfaces = local_interfaces(&arguments.interfaces, arguments.allow_public_lan)?;
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
        interfaces,
        discovery_port: arguments.discovery_port,
        requirements: vec![
            "same game and canonical typed-manifest content",
            "same runtime/core compatibility IDs, ROM, BIOS, and behavioral settings",
            "same-subnet direct RetroArch netplay with public announce, NAT traversal, and MITM relay disabled",
        ],
    };
    emit(&report, json, |value| {
        println!("LAN doctor: {}", value.status);
        println!("  discovery port: {}", value.discovery_port);
        for interface in &value.interfaces {
            println!("  interface: {interface}");
        }
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

fn lan_discover(arguments: &DiscoverArgs, json: bool) -> Result<()> {
    let sessions = discover(
        &arguments
            .discovery
            .options(Duration::from_millis(arguments.timeout_ms)),
    )?;
    emit(&sessions, json, |values| {
        if values.is_empty() {
            println!("No active LAN sessions found");
        }
        for session in values {
            println!(
                "{} game={} host={}:{} reconnect-until={}",
                session.session_id,
                session.contract.game,
                session.source.ip(),
                session.game_port,
                session.reconnect_until_unix_ms
            );
        }
    })
}

fn lan_profile(name: &str, role: NetplayRole, peer: Option<String>, port: u16) -> Result<Profile> {
    if port == 0 {
        bail!("game port must not be zero");
    }
    let mut profile = profile::load(name)?;
    if profile.runtime != RuntimeKind::Libretro {
        bail!("LAN orchestration requires a libretro/RetroArch profile");
    }
    profile.netplay.enabled = true;
    profile.netplay.role = role;
    profile.netplay.peer_address = peer;
    profile.netplay.port = port;
    profile.netplay.require_peer_contract = false;
    Ok(profile)
}

fn lan_host(arguments: &LanHostArgs, json: bool) -> Result<()> {
    if arguments.advertisement_ttl_ms == 0 || arguments.advertisement_ttl_ms > 60_000 {
        bail!("advertisement TTL must be between 1 and 60000 milliseconds");
    }
    if arguments.reconnect_seconds == 0 {
        bail!("reconnect window must be at least one second");
    }
    let profile = lan_profile(
        &arguments.profile,
        NetplayRole::Host,
        None,
        arguments.game_port,
    )?;
    let plan = runtime::prepare(&profile, arguments.game)?;
    let started = now_unix_ms();
    let session_id = format!(
        "{}-{}-{}",
        plan.session_contract.game,
        started,
        &plan.session_contract.behavior_sha256[..12]
    );
    let packet = new_packet(
        session_id,
        plan.session_contract.clone(),
        arguments.game_port,
        started,
        Duration::from_millis(arguments.advertisement_ttl_ms),
        Duration::from_secs(arguments.reconnect_seconds),
    );
    let options = arguments.discovery.options(Duration::ZERO);
    local_interfaces(&options.interfaces, options.allow_public)?;
    if json || arguments.dry_run {
        emit(
            &serde_json::json!({ "packet": &packet, "launch": &plan }),
            json,
            |value| {
                println!("session: {}", value["packet"]["session_id"]);
                println!("command: {}", plan.arguments.join(" "));
            },
        )?;
    }
    if arguments.dry_run {
        return Ok(());
    }
    advertise_once(&packet, &options)?;
    let running = Arc::new(AtomicBool::new(true));
    let advertiser_running = Arc::clone(&running);
    let advertiser_options = options.clone();
    let base_packet = packet;
    let ttl_ms = arguments.advertisement_ttl_ms;
    let reconnect_ms = arguments.reconnect_seconds.saturating_mul(1_000);
    let advertiser = thread::spawn(move || {
        while advertiser_running.load(Ordering::Relaxed) {
            let now = now_unix_ms();
            let advertisement = refreshed_advertisement(&base_packet, now, ttl_ms, reconnect_ms);
            if let Err(error) = advertise_once(&advertisement, &advertiser_options) {
                eprintln!("warning: LAN advertisement failed: {error:#}");
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
    let launch_result = runtime::launch(&plan);
    running.store(false, Ordering::Relaxed);
    advertiser
        .join()
        .map_err(|_| anyhow::anyhow!("LAN advertiser thread panicked"))?;
    let status = launch_result?;
    if !status.success() {
        bail!("runtime exited with {status}");
    }
    Ok(())
}

fn lan_join(arguments: &LanJoinArgs, json: bool) -> Result<()> {
    let options = arguments
        .discovery
        .options(Duration::from_millis(arguments.timeout_ms));
    let sessions = discover(&options)?;
    if sessions.is_empty() {
        bail!("no active LAN sessions found");
    }
    let candidate = sessions
        .iter()
        .find(|session| {
            session.contract.game == arguments.game.to_string()
                && arguments
                    .session_id
                    .as_ref()
                    .is_none_or(|id| id == &session.session_id)
        })
        .ok_or_else(|| anyhow::anyhow!("no advertised session matches game/session selection"))?;
    let peer = candidate.source.ip().to_string();
    let profile = lan_profile(
        &arguments.profile,
        NetplayRole::Client,
        Some(peer),
        candidate.game_port,
    )?;
    let plan = runtime::prepare(&profile, arguments.game)?;
    let selected = select_compatible(
        &sessions,
        &plan.session_contract,
        arguments.session_id.as_deref(),
        now_unix_ms(),
    )?;
    let profile = lan_profile(
        &arguments.profile,
        NetplayRole::Client,
        Some(selected.source.ip().to_string()),
        selected.game_port,
    )?;
    let plan = runtime::prepare(&profile, arguments.game)?;
    crate::netplay::require_agreement(&plan.session_contract, &selected.contract)?;
    if json || arguments.dry_run {
        emit(
            &serde_json::json!({ "session": selected, "launch": &plan }),
            json,
            |value| {
                println!("session: {}", value["session"]["session_id"]);
                println!("command: {}", plan.arguments.join(" "));
            },
        )?;
    }
    if arguments.dry_run {
        return Ok(());
    }
    let status = runtime::launch(&plan)?;
    if !status.success() {
        bail!("runtime exited with {status}");
    }
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

fn refreshed_advertisement(
    base: &DiscoveryPacket,
    now_ms: u64,
    ttl_ms: u64,
    reconnect_ms: u64,
) -> DiscoveryPacket {
    let mut advertisement = base.clone();
    advertisement.sent_at_unix_ms = now_ms;
    advertisement.expires_at_unix_ms = now_ms.saturating_add(ttl_ms);
    advertisement.reconnect_until_unix_ms = now_ms.saturating_add(reconnect_ms);
    advertisement
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

    #[test]
    fn lan_help_describes_public_interface_opt_in_truthfully() {
        let help = Cli::try_parse_from(["metal-slug-classics", "lan", "doctor", "--help"])
            .unwrap_err()
            .to_string();
        assert!(help.contains("Permit use of a broadcast-capable public IPv4 address"));
        assert!(help.contains("active default route"));
    }

    #[test]
    fn refreshed_advertisement_keeps_invariant_fields_unchanged() {
        let base = new_packet(
            "session-1".to_owned(),
            crate::netplay::SessionContract {
                protocol_version: 2,
                game: "3".to_owned(),
                manifest_id: "mslug3-modern".to_owned(),
                manifest_content_sha256: "manifest".to_owned(),
                runtime_compatibility_id: "runtime".to_owned(),
                core_compatibility_id: Some("core".to_owned()),
                rom_sha256: "rom".to_owned(),
                bios_sha256: "bios".to_owned(),
                behavior_sha256: "behavior".to_owned(),
            },
            DEFAULT_GAME_PORT,
            1_000_000,
            Duration::from_millis(2_000),
            Duration::from_secs(120),
        );
        let refreshed = refreshed_advertisement(&base, 2_000_000, 4_000, 300_000);
        assert_eq!(refreshed.magic, base.magic);
        assert_eq!(refreshed.discovery_version, base.discovery_version);
        assert_eq!(refreshed.session_id, base.session_id);
        assert_eq!(refreshed.started_at_unix_ms, base.started_at_unix_ms);
        assert_eq!(refreshed.game_port, base.game_port);
        assert_eq!(refreshed.contract, base.contract);
        assert_eq!(base.sent_at_unix_ms, 1_000_000);
        assert_eq!(base.expires_at_unix_ms, 1_002_000);
        assert_eq!(base.reconnect_until_unix_ms, 1_120_000);
        assert_eq!(refreshed.sent_at_unix_ms, 2_000_000);
        assert_eq!(refreshed.expires_at_unix_ms, 2_004_000);
        assert_eq!(refreshed.reconnect_until_unix_ms, 2_300_000);
    }
}
