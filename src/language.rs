#[cfg(test)]
use crate::audit::sha256_file;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub const LANGUAGE_SCHEMA_VERSION: u32 = 1;
pub const TRANSLATION_PACK_SCHEMA_VERSION: u32 = 1;
pub const MAX_LANGUAGE_IMPORT_BYTES: u64 = 64 * 1024;
pub const MAX_PACK_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TRANSLATION_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PATCH_HUNK_BYTES: usize = 64 * 1024;
const MAX_PATCH_HUNKS: usize = 16 * 1024;
const MAX_TOTAL_PATCH_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageConfig {
    pub schema_version: u32,
    pub locale: String,
    pub fallback_locale: String,
    #[serde(default)]
    pub translation_pack_sha256: Option<String>,
}

impl Default for LanguageConfig {
    fn default() -> Self {
        Self {
            schema_version: LANGUAGE_SCHEMA_VERSION,
            locale: "en".to_owned(),
            fallback_locale: "en".to_owned(),
            translation_pack_sha256: None,
        }
    }
}

impl LanguageConfig {
    /// Validates the versioned language selection and its strict fallback.
    ///
    /// # Errors
    /// Returns an error for unsupported locales, fallback chains, schema versions,
    /// or malformed translation-pack hashes.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != LANGUAGE_SCHEMA_VERSION {
            bail!(
                "unsupported language schema {}; expected {LANGUAGE_SCHEMA_VERSION}",
                self.schema_version
            );
        }
        validate_bcp47(&self.locale, "locale")?;
        if !supported_locales().contains(&self.locale.as_str()) {
            bail!(
                "unsupported launcher locale '{}'; use language list",
                self.locale
            );
        }
        validate_bcp47(&self.fallback_locale, "fallback_locale")?;
        let expected_fallback = fallback_for(&self.locale);
        if self.fallback_locale != expected_fallback {
            bail!(
                "locale '{}' requires strict fallback '{}', not '{}'",
                self.locale,
                expected_fallback,
                self.fallback_locale
            );
        }
        if let Some(hash) = &self.translation_pack_sha256 {
            validate_sha256(hash, "translation_pack_sha256")?;
        }
        validate_catalogs()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocaleReport {
    pub locale: &'static str,
    pub fallback_locale: &'static str,
    pub launcher_coverage_percent: u8,
    pub in_game: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchHunk {
    pub offset: u64,
    pub before_hex: String,
    pub after_hex: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranslationPack {
    pub schema_version: u32,
    pub format: String,
    pub game: String,
    pub locale: String,
    pub coverage_percent: u8,
    pub base_sha256: String,
    pub translated_sha256: String,
    pub file_size: u64,
    pub hunks: Vec<PatchHunk>,
}

impl TranslationPack {
    /// Validates pack structure without trusting or reading a ROM payload.
    ///
    /// # Errors
    /// Returns an error when the pack is incomplete, non-reversible, malformed,
    /// unsorted, overlapping, or not bound to exact SHA-256 identities.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != TRANSLATION_PACK_SCHEMA_VERSION {
            bail!(
                "unsupported translation-pack schema {}; expected {TRANSLATION_PACK_SCHEMA_VERSION}",
                self.schema_version
            );
        }
        if self.format != "reversible-byte-manifest-v1" {
            bail!("unsupported translation-pack format '{}'", self.format);
        }
        if self.game.is_empty() || self.game.len() > 64 {
            bail!("translation-pack game must contain 1..=64 characters");
        }
        validate_bcp47(&self.locale, "locale")?;
        if !supported_locales().contains(&self.locale.as_str()) {
            bail!("unsupported translation-pack locale '{}'", self.locale);
        }
        if self.coverage_percent != 100 {
            bail!(
                "refusing incomplete translation pack: coverage is {}%, expected 100%",
                self.coverage_percent
            );
        }
        validate_sha256(&self.base_sha256, "base_sha256")?;
        validate_sha256(&self.translated_sha256, "translated_sha256")?;
        if self.base_sha256 == self.translated_sha256 {
            bail!("translation pack does not change the base ROM");
        }
        if self.file_size == 0 || self.hunks.is_empty() {
            bail!("translation pack is incomplete: no reversible patch data");
        }
        if self.file_size > MAX_TRANSLATION_INPUT_BYTES {
            bail!("translation-pack file size exceeds {MAX_TRANSLATION_INPUT_BYTES} bytes");
        }
        if self.hunks.len() > MAX_PATCH_HUNKS {
            bail!("translation pack exceeds {MAX_PATCH_HUNKS} hunks");
        }

        let mut previous_end = 0_u64;
        let mut total_patch_bytes = 0_u64;
        for (index, hunk) in self.hunks.iter().enumerate() {
            if hunk.before_hex.len() > MAX_PATCH_HUNK_BYTES * 2
                || hunk.after_hex.len() > MAX_PATCH_HUNK_BYTES * 2
            {
                bail!("hunk {index} exceeds {MAX_PATCH_HUNK_BYTES} bytes");
            }
            let before = decode_hex(&hunk.before_hex)
                .with_context(|| format!("decode hunk {index} before_hex"))?;
            let after = decode_hex(&hunk.after_hex)
                .with_context(|| format!("decode hunk {index} after_hex"))?;
            if before.is_empty() || before.len() != after.len() {
                bail!("hunk {index} must contain equal non-empty before/after bytes");
            }
            if before.len() > MAX_PATCH_HUNK_BYTES {
                bail!("hunk {index} exceeds {MAX_PATCH_HUNK_BYTES} bytes");
            }
            if before == after {
                bail!("hunk {index} contains no change");
            }
            let length = u64::try_from(before.len()).context("patch hunk length overflow")?;
            total_patch_bytes = total_patch_bytes
                .checked_add(length)
                .ok_or_else(|| anyhow!("translation-pack patch size overflows"))?;
            if total_patch_bytes > MAX_TOTAL_PATCH_BYTES {
                bail!("translation pack exceeds {MAX_TOTAL_PATCH_BYTES} changed bytes");
            }
            let end = hunk
                .offset
                .checked_add(length)
                .ok_or_else(|| anyhow!("hunk {index} range overflows"))?;
            if index > 0 && hunk.offset < previous_end {
                bail!("hunk {index} overlaps or is not sorted");
            }
            if end > self.file_size {
                bail!("hunk {index} exceeds declared file size");
            }
            previous_end = end;
        }
        if total_patch_bytes == self.file_size {
            bail!(
                "translation pack would embed every byte of both ROM inputs; at least one byte must remain absent"
            );
        }
        Ok(())
    }

    /// Returns the canonical SHA-256 identity of the pack manifest.
    ///
    /// # Errors
    /// Returns an error when serialization fails.
    pub fn sha256(&self) -> Result<String> {
        self.validate()?;
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }
}

#[must_use]
pub fn supported_locales() -> &'static [&'static str] {
    &["en", "fr"]
}

#[must_use]
pub fn locale_reports() -> Vec<LocaleReport> {
    supported_locales()
        .iter()
        .map(|locale| LocaleReport {
            locale,
            fallback_locale: fallback_for(locale),
            launcher_coverage_percent: catalog_coverage(locale),
            in_game: if *locale == "fr" {
                "infrastructure-only; 100% is operator-declared and requires independent review"
            } else {
                "authentic Neo Geo BIOS/core option when supported by the selected region/system"
            },
        })
        .collect()
}

#[must_use]
pub const fn fallback_for(_locale: &str) -> &'static str {
    "en"
}

/// Validates canonical Neo Geo region/system profile values.
///
/// # Errors
/// Returns an error for unsupported, non-canonical, or unavailable combinations.
pub fn validate_region_system(region: &str, system: &str) -> Result<()> {
    if !matches!(region, "auto" | "europe" | "asia" | "usa" | "japan") {
        bail!("unsupported Neo Geo region '{region}'; expected auto, europe, asia, usa, or japan");
    }
    if !matches!(system, "mvs" | "aes") {
        bail!("unsupported Neo Geo system '{system}'; expected mvs or aes");
    }
    if system == "aes" && region == "usa" {
        bail!(
            "the pinned FBNeo core has no authentic AES USA mode; choose europe/asia, japan, or MVS USA"
        );
    }
    Ok(())
}

/// Looks up a launcher message with an explicit, one-step English fallback.
///
/// # Errors
/// Returns an error for an unsupported locale or a key absent from both the
/// selected catalogue and the complete English source catalogue.
pub fn message(locale: &str, key: &str) -> Result<&'static str> {
    let selected = catalog(locale).ok_or_else(|| anyhow!("unsupported locale '{locale}'"))?;
    if let Some(value) = selected.get(key) {
        return Ok(value);
    }
    let english = catalog("en").ok_or_else(|| anyhow!("English launcher catalogue is absent"))?;
    english
        .get(key)
        .copied()
        .ok_or_else(|| anyhow!("unknown launcher catalogue key '{key}'"))
}

/// Checks that every built-in catalogue has exactly the English source keys.
///
/// # Errors
/// Returns an error when the source catalogue is empty or any locale has
/// missing or unknown keys.
pub fn validate_catalogs() -> Result<()> {
    let english = catalog("en").ok_or_else(|| anyhow!("English launcher catalogue is absent"))?;
    if english.is_empty() {
        bail!("English launcher catalogue is empty");
    }
    let expected = english.keys().copied().collect::<BTreeSet<_>>();
    for locale in supported_locales() {
        let keys = catalog(locale)
            .ok_or_else(|| anyhow!("supported launcher catalogue '{locale}' is absent"))?
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        if keys != expected {
            bail!("launcher catalogue '{locale}' has missing or unknown keys");
        }
    }
    Ok(())
}

fn catalog_coverage(locale: &str) -> u8 {
    let Some(english) = catalog("en") else {
        return 0;
    };
    let Some(selected) = catalog(locale) else {
        return 0;
    };
    let covered = english
        .keys()
        .filter(|key| selected.contains_key(*key))
        .count();
    u8::try_from(covered.saturating_mul(100) / english.len()).unwrap_or(0)
}

fn catalog(locale: &str) -> Option<BTreeMap<&'static str, &'static str>> {
    let entries = match locale {
        "en" => [
            ("language.updated", "language updated"),
            ("language.valid", "language configuration is valid"),
            ("language.exported", "language configuration exported"),
            ("language.none", "no language profiles"),
            ("launcher.runtime", "runtime"),
            ("launcher.contract", "contract"),
            ("launcher.command", "command"),
        ],
        "fr" => [
            ("language.updated", "langue mise à jour"),
            ("language.valid", "la configuration de langue est valide"),
            ("language.exported", "configuration de langue exportée"),
            ("language.none", "aucun profil de langue"),
            ("launcher.runtime", "moteur"),
            ("launcher.contract", "contrat"),
            ("launcher.command", "commande"),
        ],
        _ => return None,
    };
    Some(entries.into_iter().collect())
}

/// Imports and validates a bounded language configuration.
///
/// # Errors
/// Returns an error for I/O, oversized input, malformed JSON, or invalid schema.
pub fn import(path: &Path) -> Result<LanguageConfig> {
    let bytes = bounded_read(path, MAX_LANGUAGE_IMPORT_BYTES, "language import")?;
    let value = serde_json::from_slice::<LanguageConfig>(&bytes)
        .with_context(|| format!("parse language import {}", path.display()))?;
    value.validate()?;
    Ok(value)
}

/// Exports a validated language configuration with same-directory replacement.
///
/// # Errors
/// Returns an error for invalid configuration, serialization, or I/O failure.
pub fn atomic_export(path: &Path, value: &LanguageConfig) -> Result<PathBuf> {
    value.validate()?;
    atomic_json(path, value, false)
}

/// Reads and validates a bounded translation-pack manifest.
///
/// # Errors
/// Returns an error for I/O, oversized input, malformed JSON, or invalid pack.
pub fn load_pack(path: &Path) -> Result<TranslationPack> {
    let bytes = bounded_read(path, MAX_PACK_MANIFEST_BYTES, "translation pack")?;
    let pack = serde_json::from_slice::<TranslationPack>(&bytes)
        .with_context(|| format!("parse translation pack {}", path.display()))?;
    pack.validate()?;
    Ok(pack)
}

/// Generates a reversible manifest strictly from two caller-supplied ROM files.
/// Both inputs and the output manifest must be inside the governed work root.
///
/// # Errors
/// Returns an error for paths outside work, symlinks, unequal sizes, incomplete
/// coverage, identical inputs, or an existing output.
pub fn generate_pack(
    work_root: &Path,
    base: &Path,
    translated: &Path,
    output: &Path,
    game: &str,
    locale: &str,
    coverage_percent: u8,
) -> Result<TranslationPack> {
    let root = canonical_work_root(work_root)?;
    let base = existing_work_file(&root, base, "base ROM")?;
    let translated = existing_work_file(&root, translated, "translated ROM")?;
    let output = new_work_path(&root, output, "translation-pack output")?;
    if coverage_percent != 100 {
        bail!("refusing incomplete translation pack: coverage must be exactly 100%");
    }
    let base_bytes = bounded_read(
        &base,
        MAX_TRANSLATION_INPUT_BYTES,
        "base ROM for translation pack",
    )?;
    let translated_bytes = bounded_read(
        &translated,
        MAX_TRANSLATION_INPUT_BYTES,
        "translated ROM for translation pack",
    )?;
    if base_bytes.len() != translated_bytes.len() {
        bail!("reversible byte-manifest requires equal base and translated ROM sizes");
    }
    let hunks = diff_hunks(&base_bytes, &translated_bytes)?;
    let pack = TranslationPack {
        schema_version: TRANSLATION_PACK_SCHEMA_VERSION,
        format: "reversible-byte-manifest-v1".to_owned(),
        game: game.to_owned(),
        locale: locale.to_owned(),
        coverage_percent,
        base_sha256: digest(&base_bytes),
        translated_sha256: digest(&translated_bytes),
        file_size: u64::try_from(base_bytes.len()).context("ROM size overflow")?,
        hunks,
    };
    pack.validate()?;
    let bytes = json_bytes(&pack)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PACK_MANIFEST_BYTES {
        bail!("generated translation pack exceeds {MAX_PACK_MANIFEST_BYTES} bytes");
    }
    atomic_bytes(&output, &bytes, true)?;
    Ok(pack)
}

/// Applies or reverses a validated pack after checking the exact input hash and
/// every expected byte, then publishes the result atomically without clobbering.
///
/// # Errors
/// Returns an error for paths outside work, a wrong base hash, malformed pack,
/// byte mismatch, wrong result hash, symlinks, or an existing output.
pub fn apply_pack(
    work_root: &Path,
    pack_path: &Path,
    input: &Path,
    output: &Path,
    reverse: bool,
) -> Result<String> {
    let root = canonical_work_root(work_root)?;
    let pack_path = existing_work_file(&root, pack_path, "translation pack")?;
    let input = existing_work_file(&root, input, "ROM input")?;
    let output = new_work_path(&root, output, "ROM output")?;
    let pack = load_pack(&pack_path)?;
    let mut bytes = bounded_read(&input, MAX_TRANSLATION_INPUT_BYTES, "ROM input")?;
    let expected_input = if reverse {
        &pack.translated_sha256
    } else {
        &pack.base_sha256
    };
    let expected_output = if reverse {
        &pack.base_sha256
    } else {
        &pack.translated_sha256
    };
    let actual_input = digest(&bytes);
    if actual_input != *expected_input {
        bail!(
            "wrong base hash: expected {expected_input}, got {actual_input}; output was not created"
        );
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != pack.file_size {
        bail!("ROM size does not match translation pack");
    }
    for (index, hunk) in pack.hunks.iter().enumerate() {
        let before = decode_hex(if reverse {
            &hunk.after_hex
        } else {
            &hunk.before_hex
        })?;
        let after = decode_hex(if reverse {
            &hunk.before_hex
        } else {
            &hunk.after_hex
        })?;
        let start = usize::try_from(hunk.offset).context("patch offset overflow")?;
        let end = start
            .checked_add(before.len())
            .ok_or_else(|| anyhow!("patch range overflow"))?;
        if bytes.get(start..end) != Some(before.as_slice()) {
            bail!("hunk {index} base bytes do not match; output was not created");
        }
        bytes[start..end].copy_from_slice(&after);
    }
    let actual_output = digest(&bytes);
    if actual_output != *expected_output {
        bail!("patched output hash mismatch; output was not created");
    }
    atomic_bytes(&output, &bytes, true)?;
    Ok(actual_output)
}

#[must_use]
pub fn work_root() -> PathBuf {
    env::var_os("METAL_SLUG_WORK").map_or_else(|| PathBuf::from("work"), PathBuf::from)
}

fn diff_hunks(base: &[u8], translated: &[u8]) -> Result<Vec<PatchHunk>> {
    let mut hunks = Vec::new();
    let mut total_patch_bytes = 0_u64;
    let mut index = 0_usize;
    while index < base.len() {
        if base[index] == translated[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index < base.len()
            && base[index] != translated[index]
            && index - start < MAX_PATCH_HUNK_BYTES
        {
            index += 1;
        }
        if hunks.len() == MAX_PATCH_HUNKS {
            bail!("translation pack exceeds {MAX_PATCH_HUNKS} hunks");
        }
        let length = u64::try_from(index - start).context("patch hunk length overflow")?;
        total_patch_bytes = total_patch_bytes
            .checked_add(length)
            .ok_or_else(|| anyhow!("translation-pack patch size overflows"))?;
        if total_patch_bytes > MAX_TOTAL_PATCH_BYTES {
            bail!("translation pack exceeds {MAX_TOTAL_PATCH_BYTES} changed bytes");
        }
        hunks.push(PatchHunk {
            offset: u64::try_from(start).unwrap_or(u64::MAX),
            before_hex: encode_hex(&base[start..index]),
            after_hex: encode_hex(&translated[start..index]),
        });
    }
    Ok(hunks)
}

fn canonical_work_root(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path).with_context(|| format!("create work root {}", path.display()))?;
    let root = fs::canonicalize(path)
        .with_context(|| format!("canonicalize work root {}", path.display()))?;
    if !root.is_dir() {
        bail!("work root is not a directory: {}", root.display());
    }
    Ok(root)
}

fn existing_work_file(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let (candidate, relative) = normalized_work_path(root, path, label)?;
    reject_symlink_components(root, &relative, label)?;
    let metadata = fs::symlink_metadata(&candidate)
        .with_context(|| format!("inspect {label} {}", candidate.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "{label} must be a regular non-symlink file in work: {}",
            candidate.display()
        );
    }
    let canonical = fs::canonicalize(&candidate)?;
    if !canonical.starts_with(root) {
        bail!("{label} is outside work: {}", candidate.display());
    }
    Ok(canonical)
}

fn new_work_path(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    let (candidate, relative) = normalized_work_path(root, path, label)?;
    match fs::symlink_metadata(&candidate) {
        Ok(_) => bail!("{label} already exists: {}", candidate.display()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {label} {}", candidate.display()));
        }
    }
    let parent = candidate
        .parent()
        .ok_or_else(|| anyhow!("{label} has no parent directory"))?;
    let relative_parent = relative
        .parent()
        .ok_or_else(|| anyhow!("{label} has no parent directory below work"))?;
    reject_symlink_components(root, relative_parent, label)?;
    fs::create_dir_all(parent).with_context(|| format!("create directory {}", parent.display()))?;
    reject_symlink_components(root, relative_parent, label)?;
    let canonical_parent = fs::canonicalize(parent)?;
    if !canonical_parent.starts_with(root) {
        bail!("{label} is outside work: {}", candidate.display());
    }
    let name = candidate
        .file_name()
        .ok_or_else(|| anyhow!("{label} has no file name"))?;
    Ok(canonical_parent.join(name))
}

fn normalized_work_path(root: &Path, path: &Path, label: &str) -> Result<(PathBuf, PathBuf)> {
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .with_context(|| format!("{label} is outside work: {}", path.display()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!(
            "{label} must use a normalized path below work: {}",
            path.display()
        );
    }
    Ok((root.join(&relative), relative))
}

fn reject_symlink_components(root: &Path, relative: &Path, label: &str) -> Result<()> {
    let mut candidate = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            bail!("{label} contains a non-normal path component");
        };
        candidate.push(component);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("{label} path contains a symlink: {}", candidate.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect {label} {}", candidate.display()));
            }
        }
    }
    Ok(())
}

fn bounded_read(path: &Path, maximum: u64, label: &str) -> Result<Vec<u8>> {
    let file = File::open(path).with_context(|| format!("open {label} {}", path.display()))?;
    let metadata = file.metadata()?;
    if metadata.len() > maximum {
        bail!("{label} exceeds {maximum} bytes: {}", path.display());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(maximum + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        bail!("{label} exceeds {maximum} bytes: {}", path.display());
    }
    Ok(bytes)
}

fn atomic_json<T: Serialize>(path: &Path, value: &T, no_clobber: bool) -> Result<PathBuf> {
    let bytes = json_bytes(value)?;
    atomic_bytes(path, &bytes, no_clobber)?;
    Ok(path.to_owned())
}

fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn atomic_bytes(path: &Path, bytes: &[u8], no_clobber: bool) -> Result<()> {
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if no_clobber {
        temporary
            .persist_noclobber(path)
            .map_err(|error| anyhow!("atomically publish {}: {}", path.display(), error.error))?;
    } else {
        temporary
            .persist(path)
            .map_err(|error| anyhow!("atomically publish {}: {}", path.display(), error.error))?;
    }
    sync_directory(parent)?;
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        bail!("{field} must be a canonical lowercase 64-character hexadecimal SHA-256");
    }
    Ok(())
}

fn validate_bcp47(value: &str, field: &str) -> Result<()> {
    if value.is_empty() || value.len() > 255 || !value.is_ascii() {
        bail!("{field} must be a well-formed canonical BCP 47 language tag");
    }
    let mut subtags = value.split('-');
    let language = subtags.next().unwrap_or_default();
    if !(2..=8).contains(&language.len()) || !language.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        bail!("{field} must be a well-formed canonical BCP 47 language tag");
    }
    if subtags.any(|subtag| {
        subtag.is_empty()
            || subtag.len() > 8
            || !subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }) {
        bail!("{field} must be a well-formed canonical BCP 47 language tag");
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if value.len() & 1 == 1 {
        bail!("hex byte string has odd length");
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            let high = hex_nibble(value.as_bytes()[index])?;
            let low = hex_nibble(value.as_bytes()[index + 1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => bail!("invalid hexadecimal character"),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all().map_err(Into::into)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_catalogues_have_exact_full_coverage() {
        validate_catalogs().unwrap();
        assert_eq!(catalog_coverage("en"), 100);
        assert_eq!(catalog_coverage("fr"), 100);
        assert_eq!(message("fr", "launcher.command").unwrap(), "commande");
        assert!(message("fr", "unknown").is_err());
    }

    #[test]
    fn language_validation_rejects_malformed_noncanonical_and_unsupported_bcp47_tags() {
        let language = LanguageConfig {
            locale: "fr_FR".to_owned(),
            ..LanguageConfig::default()
        };
        assert!(
            language
                .validate()
                .unwrap_err()
                .to_string()
                .contains("BCP 47")
        );

        let language = LanguageConfig {
            locale: "FR".to_owned(),
            ..LanguageConfig::default()
        };
        assert!(
            language
                .validate()
                .unwrap_err()
                .to_string()
                .contains("BCP 47")
        );

        let language = LanguageConfig {
            locale: "en-US".to_owned(),
            ..LanguageConfig::default()
        };
        assert!(
            language
                .validate()
                .unwrap_err()
                .to_string()
                .contains("unsupported launcher locale")
        );
    }

    #[test]
    fn generated_pack_is_deterministic_hash_gated_and_reversible() {
        let directory = tempfile::tempdir().unwrap();
        let work = directory.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let base = work.join("base.rom");
        let translated = work.join("translated.rom");
        fs::write(&base, b"owned base bytes").unwrap();
        fs::write(&translated, b"owned FRse bytes").unwrap();
        let first = work.join("first.pack.json");
        let second = work.join("second.pack.json");
        let pack = generate_pack(&work, &base, &translated, &first, "mslug", "fr", 100).unwrap();
        generate_pack(&work, &base, &translated, &second, "mslug", "fr", 100).unwrap();
        assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
        assert_eq!(pack.sha256().unwrap().len(), 64);

        let applied = work.join("applied.rom");
        apply_pack(&work, &first, &base, &applied, false).unwrap();
        assert_eq!(fs::read(&applied).unwrap(), fs::read(&translated).unwrap());
        let reverted = work.join("reverted.rom");
        apply_pack(&work, &first, &applied, &reverted, true).unwrap();
        assert_eq!(fs::read(&reverted).unwrap(), fs::read(&base).unwrap());
    }

    #[test]
    fn rejects_incomplete_wrong_hash_external_and_existing_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let work = directory.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let base = work.join("base.rom");
        let translated = work.join("translated.rom");
        fs::write(&base, b"base").unwrap();
        fs::write(&translated, b"FRse").unwrap();
        assert!(
            generate_pack(
                &work,
                &base,
                &translated,
                &work.join("bad.pack.json"),
                "mslug",
                "fr",
                99,
            )
            .is_err()
        );
        let pack_path = work.join("pack.json");
        generate_pack(&work, &base, &translated, &pack_path, "mslug", "fr", 100).unwrap();
        let wrong = work.join("wrong.rom");
        fs::write(&wrong, b"nope").unwrap();
        let output = work.join("output.rom");
        assert!(apply_pack(&work, &pack_path, &wrong, &output, false).is_err());
        assert!(!output.exists());
        fs::write(directory.path().join("outside.rom"), b"outside").unwrap();
        assert!(
            generate_pack(
                &work,
                &directory.path().join("outside.rom"),
                &translated,
                &work.join("outside.pack.json"),
                "mslug",
                "fr",
                100,
            )
            .is_err()
        );
        fs::write(&output, b"preserve").unwrap();
        assert!(apply_pack(&work, &pack_path, &base, &output, false).is_err());
        assert_eq!(fs::read(output).unwrap(), b"preserve");
    }

    #[test]
    fn rejects_complete_literal_payloads_and_resource_limit_bombs() {
        let directory = tempfile::tempdir().unwrap();
        let work = directory.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let base = work.join("base.rom");
        let translated = work.join("translated.rom");
        fs::write(&base, [0_u8; 32]).unwrap();
        fs::write(&translated, [1_u8; 32]).unwrap();
        let output = work.join("complete.pack.json");
        let error = generate_pack(&work, &base, &translated, &output, "mslug", "fr", 100)
            .unwrap_err()
            .to_string();
        assert!(error.contains("embed every byte"));
        assert!(!output.exists());

        let oversized = work.join("oversized.rom");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_TRANSLATION_INPUT_BYTES + 1)
            .unwrap();
        let error = generate_pack(
            &work,
            &oversized,
            &translated,
            &work.join("oversized.pack.json"),
            "mslug",
            "fr",
            100,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("exceeds"));

        let too_many_hunks = TranslationPack {
            schema_version: TRANSLATION_PACK_SCHEMA_VERSION,
            format: "reversible-byte-manifest-v1".to_owned(),
            game: "mslug".to_owned(),
            locale: "fr".to_owned(),
            coverage_percent: 100,
            base_sha256: "0".repeat(64),
            translated_sha256: "1".repeat(64),
            file_size: u64::try_from((MAX_PATCH_HUNKS + 1) * 2).unwrap(),
            hunks: (0..=MAX_PATCH_HUNKS)
                .map(|index| PatchHunk {
                    offset: u64::try_from(index * 2).unwrap(),
                    before_hex: "00".to_owned(),
                    after_hex: "01".to_owned(),
                })
                .collect(),
        };
        assert!(
            too_many_hunks
                .validate()
                .unwrap_err()
                .to_string()
                .contains("hunks")
        );
    }

    #[test]
    fn traversal_and_tampered_output_hash_leave_no_output() {
        let directory = tempfile::tempdir().unwrap();
        let work = directory.path().join("work");
        fs::create_dir_all(&work).unwrap();
        let base = work.join("base.rom");
        let translated = work.join("translated.rom");
        fs::write(&base, b"owned base").unwrap();
        fs::write(&translated, b"owned FRse").unwrap();

        let escaped_parent = directory.path().join("escaped");
        let error = generate_pack(
            &work,
            &base,
            &translated,
            Path::new("../escaped/pack.json"),
            "mslug",
            "fr",
            100,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("normalized path"));
        assert!(!escaped_parent.exists());

        let pack_path = work.join("pack.json");
        let mut pack =
            generate_pack(&work, &base, &translated, &pack_path, "mslug", "fr", 100).unwrap();
        pack.translated_sha256 = "f".repeat(64);
        let tampered = work.join("tampered.pack.json");
        atomic_json(&tampered, &pack, true).unwrap();
        let output = work.join("must-not-exist.rom");
        let error = apply_pack(&work, &tampered, &base, &output, false)
            .unwrap_err()
            .to_string();
        assert!(error.contains("output hash mismatch"));
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_inputs_and_output_parents() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let work = directory.path().join("work");
        let outside = directory.path().join("outside");
        fs::create_dir_all(&work).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let real_base = work.join("base.rom");
        let translated = work.join("translated.rom");
        fs::write(&real_base, b"owned base").unwrap();
        fs::write(&translated, b"owned FRse").unwrap();
        let linked_base = work.join("linked-base.rom");
        symlink(&real_base, &linked_base).unwrap();
        assert!(
            generate_pack(
                &work,
                &linked_base,
                &translated,
                &work.join("linked.pack.json"),
                "mslug",
                "fr",
                100,
            )
            .is_err()
        );

        let linked_parent = work.join("linked-parent");
        symlink(&outside, &linked_parent).unwrap();
        assert!(
            generate_pack(
                &work,
                &real_base,
                &translated,
                &linked_parent.join("new-directory/pack.json"),
                "mslug",
                "fr",
                100,
            )
            .is_err()
        );
        assert!(!outside.join("new-directory").exists());
    }

    #[test]
    fn sha256_file_identity_matches_pack_digest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample");
        fs::write(&path, b"sample").unwrap();
        assert_eq!(sha256_file(&path).unwrap(), digest(b"sample"));
    }
}
