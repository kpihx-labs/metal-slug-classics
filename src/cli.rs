use crate::audit::{ArchiveAudit, AuditReport, MemberState, audit_path};
use crate::controls::{
    self, ControlsConfig, ControlsPreset, DeviceKind, InputBinding, MappingUpdate,
};
use crate::language::{self, LanguageConfig};
use crate::manifest::{GameId, manifests};
use crate::netplay::{
    AgreementReport, DEFAULT_DISCOVERY_PORT, DEFAULT_GAME_PORT, DEFAULT_MULTICAST_ADDRESS,
    DiscoveryOptions, DiscoveryPacket, advertise_once, compare, discover, load as load_contract,
    local_interfaces, new_packet, now_unix_ms, select_compatible,
};
use crate::normalize::{NormalizeReport, normalize_path};
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
    Normalize {
        source: PathBuf,
        #[arg(
            long,
            value_name = "DIRECTORY",
            help = "Explicit output directory outside the source"
        )]
        output: PathBuf,
        #[arg(long, help = "Atomically replace an existing output directory")]
        force: bool,
        #[arg(
            long,
            help = "Report and skip incomplete/corrupt archives in a collection"
        )]
        skip_blocked: bool,
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
    Controls {
        #[command(subcommand)]
        command: ControlsCommand,
    },
    Language {
        #[command(subcommand)]
        command: LanguageCommand,
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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ControlsPresetArgument {
    Authentic,
    Modern,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum LanguageArgument {
    En,
    Fr,
}

impl LanguageArgument {
    const fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RegionArgument {
    Auto,
    Europe,
    Asia,
    Usa,
    Japan,
}

impl RegionArgument {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Europe => "europe",
            Self::Asia => "asia",
            Self::Usa => "usa",
            Self::Japan => "japan",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SystemArgument {
    Mvs,
    Aes,
}

impl SystemArgument {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Mvs => "mvs",
            Self::Aes => "aes",
        }
    }
}

impl From<ControlsPresetArgument> for ControlsPreset {
    fn from(value: ControlsPresetArgument) -> Self {
        match value {
            ControlsPresetArgument::Authentic => Self::Authentic,
            ControlsPresetArgument::Modern => Self::Modern,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DeviceKindArgument {
    Keyboard,
    Gamepad,
}

impl From<DeviceKindArgument> for DeviceKind {
    fn from(value: DeviceKindArgument) -> Self {
        match value {
            DeviceKindArgument::Keyboard => Self::Keyboard,
            DeviceKindArgument::Gamepad => Self::Gamepad,
        }
    }
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
    #[arg(
        long,
        value_enum,
        help = "Install an explicit optional controls preset"
    )]
    controls_preset: Option<ControlsPresetArgument>,
    #[arg(long, value_enum, default_value = "en")]
    language: LanguageArgument,
    #[arg(long, value_enum, default_value = "auto")]
    region: RegionArgument,
    #[arg(long, value_enum, default_value = "mvs")]
    system: SystemArgument,
}

#[derive(Debug, Subcommand)]
enum ControlsCommand {
    Show(ControlsProfileArgs),
    Set(ControlsSetArgs),
    Reset(ControlsResetArgs),
    Import(ControlsImportArgs),
    Export(ControlsExportArgs),
    Validate(ControlsValidateArgs),
}

#[derive(Debug, Args)]
struct ControlsProfileArgs {
    #[arg(default_value = "default")]
    profile: String,
}

#[derive(Debug, Args)]
struct ControlsSetArgs {
    #[arg(default_value = "default")]
    profile: String,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
    player: u8,
    #[arg(long)]
    device: String,
    #[arg(long, value_enum)]
    kind: DeviceKindArgument,
    #[arg(long, default_value_t = 0)]
    index: u8,
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u16).range(0..=950))]
    deadzone: u16,
    #[arg(long, help = "ACTION or hotkey.NAME")]
    target: String,
    #[arg(
        long,
        required_unless_present = "clear",
        conflicts_with = "clear",
        help = "key:NAME[:FBNEO_CODE], button:N, or axis:N:+|-:THRESHOLD"
    )]
    binding: Option<InputBinding>,
    #[arg(long, help = "Remove the target mapping from this device")]
    clear: bool,
}

#[derive(Debug, Args)]
struct ControlsResetArgs {
    #[arg(default_value = "default")]
    profile: String,
    #[arg(long, value_enum)]
    preset: Option<ControlsPresetArgument>,
}

#[derive(Debug, Args)]
struct ControlsImportArgs {
    input: PathBuf,
    #[arg(long, default_value = "default")]
    profile: String,
}

#[derive(Debug, Args)]
struct ControlsExportArgs {
    output: PathBuf,
    #[arg(long, default_value = "default")]
    profile: String,
}

#[derive(Debug, Args)]
struct ControlsValidateArgs {
    #[arg(default_value = "default")]
    profile: String,
    #[arg(
        long,
        help = "Validate an exported controls JSON file instead of the profile"
    )]
    input: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum LanguageCommand {
    List,
    Show(LanguageProfileArgs),
    Set(LanguageSetArgs),
    Import(LanguageImportArgs),
    Export(LanguageExportArgs),
    Validate(LanguageValidateArgs),
    Pack {
        #[command(subcommand)]
        command: TranslationPackCommand,
    },
}

#[derive(Debug, Args)]
struct LanguageProfileArgs {
    #[arg(default_value = "default")]
    profile: String,
}

#[derive(Debug, Args)]
struct LanguageSetArgs {
    #[arg(default_value = "default")]
    profile: String,
    #[arg(long, value_enum)]
    locale: LanguageArgument,
    #[arg(long, value_enum)]
    region: Option<RegionArgument>,
    #[arg(long, value_enum)]
    system: Option<SystemArgument>,
    #[arg(long, conflicts_with = "clear_translation_pack")]
    translation_pack: Option<PathBuf>,
    #[arg(long)]
    clear_translation_pack: bool,
}

#[derive(Debug, Args)]
struct LanguageImportArgs {
    input: PathBuf,
    #[arg(long, default_value = "default")]
    profile: String,
}

#[derive(Debug, Args)]
struct LanguageExportArgs {
    output: PathBuf,
    #[arg(long, default_value = "default")]
    profile: String,
}

#[derive(Debug, Args)]
struct LanguageValidateArgs {
    #[arg(default_value = "default")]
    profile: String,
    #[arg(long)]
    input: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum TranslationPackCommand {
    Generate(TranslationPackGenerateArgs),
    Validate(TranslationPackValidateArgs),
    Apply(TranslationPackApplyArgs),
    Revert(TranslationPackApplyArgs),
}

#[derive(Debug, Args)]
struct TranslationPackGenerateArgs {
    base: PathBuf,
    translated: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    game: String,
    #[arg(long, value_enum)]
    locale: LanguageArgument,
    #[arg(long, default_value_t = 100)]
    coverage: u8,
}

#[derive(Debug, Args)]
struct TranslationPackValidateArgs {
    pack: PathBuf,
}

#[derive(Debug, Args)]
struct TranslationPackApplyArgs {
    pack: PathBuf,
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
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
        Command::Normalize {
            source,
            output,
            force,
            skip_blocked,
        } => normalize(&source, &output, force, skip_blocked, cli.json)
            .map_err(|error| command_failure("normalize-failed", &error)),
        Command::Play {
            game,
            profile,
            dry_run,
        } => play(game, &profile, dry_run, cli.json)
            .map_err(|error| command_failure("play-failed", &error)),
        Command::Profile { command } => profile_command(command, cli.json)
            .map_err(|error| command_failure("profile-failed", &error)),
        Command::Controls { command } => controls_command(command, cli.json)
            .map_err(|error| command_failure("controls-failed", &error)),
        Command::Language { command } => language_command(command, cli.json)
            .map_err(|error| command_failure("language-failed", &error)),
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

fn normalize(
    source: &std::path::Path,
    output: &std::path::Path,
    force: bool,
    skip_blocked: bool,
    json: bool,
) -> Result<()> {
    let report = normalize_path(source, output, force, skip_blocked)?;
    emit(&report, json, print_normalize)
}

fn print_normalize(report: &NormalizeReport) {
    println!("Normalized output: {}", report.output.display());
    for archive in &report.archives {
        println!(
            "  {} ({}, SHA-256 {})",
            archive.archive.display(),
            archive.manifest_id,
            archive.output_sha256
        );
        println!("    provenance: {}", archive.provenance.display());
    }
    for skipped in &report.skipped {
        println!(
            "  skipped {} ({:?})",
            skipped.archive.display(),
            skipped.classification
        );
    }
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
    let locale = profile.language.locale.clone();
    let plan = runtime::prepare(&profile, game)?;
    if json || dry_run {
        emit(&plan, json, |value| {
            println!(
                "{}: {}",
                language::message(&locale, "launcher.runtime").expect("catalogue key"),
                value.program.display()
            );
            println!(
                "{}: {}",
                language::message(&locale, "launcher.contract").expect("catalogue key"),
                value.contract_path.display()
            );
            println!(
                "{}: {} {}",
                language::message(&locale, "launcher.command").expect("catalogue key"),
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
                controls_preset,
                language: profile_language,
                region,
                system,
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
            value.language = LanguageConfig {
                locale: profile_language.as_str().to_owned(),
                fallback_locale: language::fallback_for(profile_language.as_str()).to_owned(),
                ..LanguageConfig::default()
            };
            region.as_str().clone_into(&mut value.region);
            system.as_str().clone_into(&mut value.system);
            if let Some(preset) = controls_preset {
                value.controls = ControlsConfig::preset(preset.into());
            }
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

fn controls_command(command: ControlsCommand, json: bool) -> Result<()> {
    match command {
        ControlsCommand::Show(arguments) => {
            let value = profile::load(&arguments.profile)?.controls;
            emit(&value, json, |controls| {
                println!(
                    "{}",
                    serde_json::to_string_pretty(controls).expect("serialize controls")
                );
            })
        }
        ControlsCommand::Set(arguments) => {
            let mut value = profile::load(&arguments.profile)?;
            if arguments.clear {
                value
                    .controls
                    .clear(arguments.player, &arguments.device, &arguments.target)?;
            } else {
                value.controls.set(MappingUpdate {
                    player: arguments.player,
                    device_id: &arguments.device,
                    kind: arguments.kind.into(),
                    index: arguments.index,
                    deadzone_per_mille: arguments.deadzone,
                    target: &arguments.target,
                    binding: arguments
                        .binding
                        .expect("clap requires binding unless --clear"),
                })?;
            }
            let path = profile::replace(&value)?;
            emit_controls_result(&value.controls, &path, json)
        }
        ControlsCommand::Reset(arguments) => {
            let mut value = profile::load(&arguments.profile)?;
            value.controls = arguments
                .preset
                .map_or_else(ControlsConfig::default, |preset| {
                    ControlsConfig::preset(preset.into())
                });
            let path = profile::replace(&value)?;
            emit_controls_result(&value.controls, &path, json)
        }
        ControlsCommand::Import(arguments) => {
            let mut value = profile::load(&arguments.profile)?;
            value.controls = controls::import(&arguments.input)?;
            let path = profile::replace(&value)?;
            emit_controls_result(&value.controls, &path, json)
        }
        ControlsCommand::Export(arguments) => {
            let value = profile::load(&arguments.profile)?;
            let path = controls::atomic_export(&arguments.output, &value.controls)?;
            emit_controls_result(&value.controls, &path, json)
        }
        ControlsCommand::Validate(arguments) => {
            let value = if let Some(path) = arguments.input {
                controls::import(&path)?
            } else {
                profile::load(&arguments.profile)?.controls
            };
            value.validate()?;
            emit(
                &serde_json::json!({ "valid": true, "schema_version": value.schema_version }),
                json,
                |_| println!("controls are valid (schema {})", value.schema_version),
            )
        }
    }
}

fn emit_controls_result(controls: &ControlsConfig, path: &PathBuf, json: bool) -> Result<()> {
    let validation_error = controls.validate().err().map(|error| error.to_string());
    let result = serde_json::json!({
        "path": path,
        "controls": controls,
        "valid": validation_error.is_none(),
        "validation_error": validation_error,
    });
    emit(&result, json, |value| {
        println!("updated {}", value["path"].as_str().unwrap_or("controls"));
        if let Some(error) = value["validation_error"].as_str() {
            println!("draft is not launch-ready: {error}");
        }
    })
}

fn language_command(command: LanguageCommand, json: bool) -> Result<()> {
    match command {
        LanguageCommand::List => {
            let reports = language::locale_reports();
            emit(&reports, json, |values| {
                for value in values {
                    println!(
                        "{}: launcher {}%, fallback={}, in-game={}",
                        value.locale,
                        value.launcher_coverage_percent,
                        value.fallback_locale,
                        value.in_game
                    );
                }
            })
        }
        LanguageCommand::Show(arguments) => {
            let profile = profile::load(&arguments.profile)?;
            let report = serde_json::json!({
                "language": profile.language,
                "region": profile.region,
                "system": profile.system,
            });
            emit(&report, json, |value| {
                println!(
                    "{}",
                    serde_json::to_string_pretty(value).expect("serialize language")
                );
            })
        }
        LanguageCommand::Set(arguments) => {
            let mut profile = profile::load(&arguments.profile)?;
            let locale = arguments.locale.as_str();
            let translation_pack_sha256 = if let Some(path) = arguments.translation_pack {
                let pack = language::load_pack(&path)?;
                if pack.locale != locale {
                    bail!(
                        "translation pack locale '{}' does not match requested locale '{locale}'",
                        pack.locale
                    );
                }
                Some(pack.sha256()?)
            } else if arguments.clear_translation_pack {
                None
            } else if profile.language.locale == locale {
                profile.language.translation_pack_sha256.clone()
            } else {
                None
            };
            profile.language = LanguageConfig {
                schema_version: language::LANGUAGE_SCHEMA_VERSION,
                locale: locale.to_owned(),
                fallback_locale: language::fallback_for(locale).to_owned(),
                translation_pack_sha256,
            };
            if let Some(region) = arguments.region {
                region.as_str().clone_into(&mut profile.region);
            }
            if let Some(system) = arguments.system {
                system.as_str().clone_into(&mut profile.system);
            }
            profile.language.validate()?;
            let path = profile::replace(&profile)?;
            emit_language_result(&profile, &path, json, "language.updated")
        }
        LanguageCommand::Import(arguments) => {
            let mut profile = profile::load(&arguments.profile)?;
            profile.language = language::import(&arguments.input)?;
            let path = profile::replace(&profile)?;
            emit_language_result(&profile, &path, json, "language.updated")
        }
        LanguageCommand::Export(arguments) => {
            let profile = profile::load(&arguments.profile)?;
            let path = language::atomic_export(&arguments.output, &profile.language)?;
            emit_language_result(&profile, &path, json, "language.exported")
        }
        LanguageCommand::Validate(arguments) => {
            let value = if let Some(path) = arguments.input {
                language::import(&path)?
            } else {
                profile::load(&arguments.profile)?.language
            };
            value.validate()?;
            emit(
                &serde_json::json!({
                    "valid": true,
                    "schema_version": value.schema_version,
                    "locale": value.locale,
                    "fallback_locale": value.fallback_locale,
                }),
                json,
                |_| {
                    println!(
                        "{}",
                        language::message(&value.locale, "language.valid").expect("catalogue key")
                    );
                },
            )
        }
        LanguageCommand::Pack { command } => translation_pack_command(command, json),
    }
}

fn emit_language_result(
    profile: &Profile,
    path: &std::path::Path,
    json: bool,
    message_key: &str,
) -> Result<()> {
    let report = serde_json::json!({
        "path": path,
        "language": profile.language,
        "region": profile.region,
        "system": profile.system,
    });
    emit(&report, json, |_| {
        println!(
            "{}: {}",
            language::message(&profile.language.locale, message_key).expect("catalogue key"),
            path.display()
        );
    })
}

fn translation_pack_command(command: TranslationPackCommand, json: bool) -> Result<()> {
    match command {
        TranslationPackCommand::Generate(arguments) => {
            let pack = language::generate_pack(
                &language::work_root(),
                &arguments.base,
                &arguments.translated,
                &arguments.output,
                &arguments.game,
                arguments.locale.as_str(),
                arguments.coverage,
            )?;
            emit(&pack, json, |value| {
                println!(
                    "translation pack generated: {} hunks, SHA-256 {}",
                    value.hunks.len(),
                    value.sha256().expect("validated pack")
                );
            })
        }
        TranslationPackCommand::Validate(arguments) => {
            let pack = language::load_pack(&arguments.pack)?;
            emit(
                &serde_json::json!({
                    "valid": true,
                    "pack_sha256": pack.sha256()?,
                    "base_sha256": pack.base_sha256,
                    "translated_sha256": pack.translated_sha256,
                    "coverage_percent": pack.coverage_percent,
                }),
                json,
                |_| {
                    println!(
                        "translation pack is structurally valid; 100% coverage is operator-declared"
                    );
                },
            )
        }
        TranslationPackCommand::Apply(arguments) => {
            let hash = language::apply_pack(
                &language::work_root(),
                &arguments.pack,
                &arguments.input,
                &arguments.output,
                false,
            )?;
            emit(
                &serde_json::json!({ "output": arguments.output, "sha256": hash }),
                json,
                |value| println!("applied translation pack: SHA-256 {}", value["sha256"]),
            )
        }
        TranslationPackCommand::Revert(arguments) => {
            let hash = language::apply_pack(
                &language::work_root(),
                &arguments.pack,
                &arguments.input,
                &arguments.output,
                true,
            )?;
            emit(
                &serde_json::json!({ "output": arguments.output, "sha256": hash }),
                json,
                |value| println!("reverted translation pack: SHA-256 {}", value["sha256"]),
            )
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
                protocol_version: 3,
                game: "3".to_owned(),
                manifest_id: "mslug3-modern".to_owned(),
                manifest_content_sha256: "manifest".to_owned(),
                runtime_compatibility_id: "runtime".to_owned(),
                core_compatibility_id: Some("core".to_owned()),
                rom_sha256: "rom".to_owned(),
                bios_sha256: "bios".to_owned(),
                language: "en".to_owned(),
                region: "europe".to_owned(),
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
