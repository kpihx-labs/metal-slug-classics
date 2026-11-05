use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeKind {
    Standalone,
    #[default]
    Libretro,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetplayRole {
    #[default]
    Host,
    Client,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NetplaySettings {
    pub enabled: bool,
    #[serde(default)]
    pub role: NetplayRole,
    #[serde(default, alias = "host")]
    pub peer_address: Option<String>,
    pub port: u16,
    pub require_peer_contract: bool,
}

impl Default for NetplaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            role: NetplayRole::Host,
            peer_address: None,
            port: 55_435,
            require_peer_contract: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub runtime: RuntimeKind,
    pub rom_directory: PathBuf,
    pub bios_archive: PathBuf,
    pub standalone_executable: Option<PathBuf>,
    pub retroarch_executable: Option<PathBuf>,
    pub libretro_core: Option<PathBuf>,
    #[serde(default)]
    pub runtime_compatibility_id: String,
    #[serde(default)]
    pub core_compatibility_id: Option<String>,
    #[serde(default)]
    pub expected_runtime_sha256: Option<String>,
    #[serde(default)]
    pub expected_core_sha256: Option<String>,
    pub region: String,
    pub system: String,
    pub authentic_slowdown: bool,
    pub netplay: NetplaySettings,
}

impl Profile {
    /// Builds a profile with portable defaults.
    ///
    /// # Errors
    /// Returns an error when the profile name is unsafe.
    pub fn new(name: &str, rom_directory: PathBuf) -> Result<Self> {
        validate_name(name)?;
        Ok(Self {
            name: name.to_owned(),
            runtime: RuntimeKind::Libretro,
            bios_archive: rom_directory.join("neogeo.zip"),
            rom_directory,
            standalone_executable: None,
            retroarch_executable: None,
            libretro_core: None,
            runtime_compatibility_id: String::new(),
            core_compatibility_id: None,
            expected_runtime_sha256: None,
            expected_core_sha256: None,
            region: "auto".to_owned(),
            system: "mvs".to_owned(),
            authentic_slowdown: true,
            netplay: NetplaySettings::default(),
        })
    }

    /// Serializes the profile into canonical compact JSON.
    ///
    /// # Errors
    /// Returns an error if serialization fails.
    pub fn canonical_json(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context("serialize profile")
    }

    /// Hashes the canonical profile representation.
    ///
    /// # Errors
    /// Returns an error if profile serialization fails.
    pub fn sha256(&self) -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(self.canonical_json()?)))
    }

    /// Hashes only settings that can affect deterministic emulation behavior.
    /// Machine-local paths and LAN locator/role fields are deliberately excluded.
    ///
    /// # Errors
    /// Returns an error if the behavioral settings cannot be serialized.
    pub fn behavior_sha256(&self) -> Result<String> {
        let core_compatibility_id = match self.runtime {
            RuntimeKind::Standalone => None,
            RuntimeKind::Libretro => self.core_compatibility_id.as_deref(),
        };
        let canonical = serde_json::to_vec(&(
            1_u32,
            self.runtime,
            &self.runtime_compatibility_id,
            core_compatibility_id,
            &self.region,
            &self.system,
            self.authentic_slowdown,
        ))
        .context("serialize behavioral profile")?;
        Ok(format!("{:x}", Sha256::digest(canonical)))
    }
}

/// Resolves the platform-native application configuration root.
///
/// # Errors
/// Returns an error when the required platform home variable is unavailable.
pub fn config_root() -> Result<PathBuf> {
    config_root_with(env::consts::OS, |key| env::var_os(key))
}

fn config_root_with<F>(os: &str, getenv: F) -> Result<PathBuf>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    if let Some(explicit) = getenv("METAL_SLUG_CONFIG_HOME") {
        return Ok(PathBuf::from(explicit));
    }
    match os {
        "windows" => getenv("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("MetalSlugClassics"))
            .ok_or_else(|| anyhow!("APPDATA is not set")),
        "macos" => home_from(&getenv)
            .map(|path| path.join("Library/Application Support/MetalSlugClassics")),
        _ => {
            if let Some(xdg) = getenv("XDG_CONFIG_HOME") {
                Ok(PathBuf::from(xdg).join("metal-slug-classics"))
            } else {
                home_from(&getenv).map(|path| path.join(".config/metal-slug-classics"))
            }
        }
    }
}

fn home_from<F>(getenv: &F) -> Result<PathBuf>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    getenv("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

/// Returns the profile storage directory.
///
/// # Errors
/// Returns an error when the platform configuration root cannot be resolved.
pub fn profiles_dir() -> Result<PathBuf> {
    Ok(config_root()?.join("profiles"))
}

/// Returns the safe path for a named profile.
///
/// # Errors
/// Returns an error for an unsafe name or unavailable configuration root.
pub fn profile_path(name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(profiles_dir()?.join(format!("{name}.json")))
}

/// Creates a new profile without overwriting an existing one.
///
/// # Errors
/// Returns an error for invalid data or any directory/file write failure.
pub fn create(profile: &Profile) -> Result<PathBuf> {
    validate_name(&profile.name)?;
    let directory = profiles_dir()?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("create profile directory {}", directory.display()))?;
    let path = directory.join(format!("{}.json", profile.name));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("create profile {} (refusing to overwrite)", path.display()))?;
    let mut bytes = serde_json::to_vec_pretty(profile)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    Ok(path)
}

/// Loads and validates a named profile.
///
/// # Errors
/// Returns an error when the profile is absent, invalid, or name-mismatched.
pub fn load(name: &str) -> Result<Profile> {
    let path = profile_path(name)?;
    let bytes = fs::read(&path).with_context(|| format!("read profile {}", path.display()))?;
    let profile: Profile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse profile {}", path.display()))?;
    validate_name(&profile.name)?;
    if profile.name != name {
        bail!(
            "profile file/name mismatch: requested '{name}', contains '{}'",
            profile.name
        );
    }
    Ok(profile)
}

/// Lists profile names in deterministic order.
///
/// # Errors
/// Returns an error when the profile directory cannot be read.
pub fn list() -> Result<Vec<String>> {
    let directory = profiles_dir()?;
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut names = fs::read_dir(&directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .filter_map(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

/// Resolves the initial ROM directory for profile creation.
///
/// # Errors
/// Returns an error when the current directory cannot be determined.
pub fn default_rom_directory() -> Result<PathBuf> {
    if let Some(path) = env::var_os("METAL_SLUG_ROMS") {
        return Ok(PathBuf::from(path));
    }
    Ok(env::current_dir()?.join("roms"))
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, b'-' | b'_'))
    {
        bail!("profile name must be 1-64 ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

#[must_use]
pub fn portable_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn vars<'a>(values: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            values
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn computes_linux_xdg_path() {
        let root = config_root_with("linux", vars(&[("XDG_CONFIG_HOME", "/tmp/config")])).unwrap();
        assert_eq!(root, Path::new("/tmp/config/metal-slug-classics"));
    }

    #[test]
    fn computes_windows_roaming_path() {
        let root = config_root_with(
            "windows",
            vars(&[("APPDATA", r"C:\Users\K\AppData\Roaming")]),
        )
        .unwrap();
        assert_eq!(
            root,
            Path::new(r"C:\Users\K\AppData\Roaming").join("MetalSlugClassics")
        );
    }

    #[test]
    fn rejects_traversal_names() {
        assert!(Profile::new("../escape", PathBuf::from("roms")).is_err());
    }

    #[test]
    fn behavioral_hash_excludes_paths_and_netplay_locator_role() {
        let mut host = Profile::new("host", PathBuf::from("/linux/roms")).unwrap();
        host.bios_archive = PathBuf::from("/linux/bios/neogeo.zip");
        host.retroarch_executable = Some(PathBuf::from("/usr/bin/retroarch"));
        host.libretro_core = Some(PathBuf::from("/linux/fbneo.so"));
        host.runtime_compatibility_id = "retroarch-1.20-netplay-v1".to_owned();
        host.core_compatibility_id = Some("fbneo-custom-2026-11-v1".to_owned());
        host.expected_runtime_sha256 = Some("1".repeat(64));
        host.expected_core_sha256 = Some("2".repeat(64));
        host.netplay.enabled = true;
        host.netplay.role = NetplayRole::Host;
        host.netplay.port = 55_435;

        let mut client = host.clone();
        client.name = "client".to_owned();
        client.rom_directory = PathBuf::from(r"D:\Games\roms");
        client.bios_archive = PathBuf::from(r"D:\Games\bios\neogeo.zip");
        client.retroarch_executable = Some(PathBuf::from(r"C:\RetroArch\retroarch.exe"));
        client.libretro_core = Some(PathBuf::from(r"C:\RetroArch\cores\fbneo.dll"));
        client.expected_runtime_sha256 = Some("3".repeat(64));
        client.expected_core_sha256 = Some("4".repeat(64));
        client.netplay.role = NetplayRole::Client;
        client.netplay.peer_address = Some("192.0.2.10".to_owned());
        client.netplay.port = 60_000;

        assert_ne!(host.sha256().unwrap(), client.sha256().unwrap());
        assert_eq!(
            host.behavior_sha256().unwrap(),
            client.behavior_sha256().unwrap()
        );
    }
}
