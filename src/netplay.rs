use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionContract {
    pub protocol_version: u32,
    pub game: String,
    pub manifest_id: String,
    pub manifest_content_sha256: String,
    pub runtime_compatibility_id: String,
    pub core_compatibility_id: Option<String>,
    pub rom_sha256: String,
    pub bios_sha256: String,
    pub behavior_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContractMismatch {
    pub field: String,
    pub local: String,
    pub peer: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgreementReport {
    pub agreed: bool,
    pub mismatches: Vec<ContractMismatch>,
}

#[must_use]
pub fn compare(local: &SessionContract, peer: &SessionContract) -> AgreementReport {
    let mut mismatches = Vec::new();
    compare_field(
        &mut mismatches,
        "protocol_version",
        local.protocol_version.to_string(),
        peer.protocol_version.to_string(),
    );
    compare_field(
        &mut mismatches,
        "game",
        local.game.clone(),
        peer.game.clone(),
    );
    compare_field(
        &mut mismatches,
        "manifest_id",
        local.manifest_id.clone(),
        peer.manifest_id.clone(),
    );
    compare_field(
        &mut mismatches,
        "manifest_content_sha256",
        local.manifest_content_sha256.clone(),
        peer.manifest_content_sha256.clone(),
    );
    compare_field(
        &mut mismatches,
        "runtime_compatibility_id",
        local.runtime_compatibility_id.clone(),
        peer.runtime_compatibility_id.clone(),
    );
    compare_field(
        &mut mismatches,
        "core_compatibility_id",
        local
            .core_compatibility_id
            .as_deref()
            .unwrap_or("none")
            .to_owned(),
        peer.core_compatibility_id
            .as_deref()
            .unwrap_or("none")
            .to_owned(),
    );
    compare_field(
        &mut mismatches,
        "rom_sha256",
        local.rom_sha256.clone(),
        peer.rom_sha256.clone(),
    );
    compare_field(
        &mut mismatches,
        "bios_sha256",
        local.bios_sha256.clone(),
        peer.bios_sha256.clone(),
    );
    compare_field(
        &mut mismatches,
        "behavior_sha256",
        local.behavior_sha256.clone(),
        peer.behavior_sha256.clone(),
    );
    AgreementReport {
        agreed: mismatches.is_empty(),
        mismatches,
    }
}

/// Enforces byte-for-byte agreement of every prelaunch contract field.
///
/// # Errors
/// Returns an error listing every field whose value differs.
pub fn require_agreement(local: &SessionContract, peer: &SessionContract) -> Result<()> {
    let report = compare(local, peer);
    if report.agreed {
        return Ok(());
    }
    let fields = report
        .mismatches
        .iter()
        .map(|mismatch| mismatch.field.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("LAN prelaunch hash agreement failed: {fields}")
}

/// Loads a JSON session contract.
///
/// # Errors
/// Returns an error when the file cannot be read or parsed.
pub fn load(path: &Path) -> Result<SessionContract> {
    let bytes = fs::read(path).with_context(|| format!("read LAN contract {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse LAN contract {}", path.display()))
}

fn compare_field(mismatches: &mut Vec<ContractMismatch>, field: &str, local: String, peer: String) {
    if local != peer {
        mismatches.push(ContractMismatch {
            field: field.to_owned(),
            local,
            peer,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> SessionContract {
        SessionContract {
            protocol_version: 1,
            game: "3".to_owned(),
            manifest_id: "mslug3-modern".to_owned(),
            manifest_content_sha256: "manifest-content".to_owned(),
            runtime_compatibility_id: "retroarch-netplay-v1".to_owned(),
            core_compatibility_id: Some("fbneo-custom-v1".to_owned()),
            rom_sha256: "rom".to_owned(),
            bios_sha256: "bios".to_owned(),
            behavior_sha256: "behavior".to_owned(),
        }
    }

    #[test]
    fn exact_contracts_agree() {
        let local = contract();
        assert!(compare(&local, &local).agreed);
        assert!(require_agreement(&local, &local).is_ok());
    }

    #[test]
    fn any_hash_mismatch_blocks_prelaunch() {
        let local = contract();
        let mut peer = contract();
        peer.bios_sha256 = "different".to_owned();
        let report = compare(&local, &peer);
        assert!(!report.agreed);
        assert_eq!(report.mismatches[0].field, "bios_sha256");
        assert!(require_agreement(&local, &peer).is_err());
    }

    #[test]
    fn reports_all_mismatches_in_stable_field_order() {
        let local = contract();
        let mut peer = contract();
        peer.core_compatibility_id = Some("other-core".to_owned());
        peer.rom_sha256 = "other-rom".to_owned();
        peer.behavior_sha256 = "other-behavior".to_owned();
        let fields = compare(&local, &peer)
            .mismatches
            .into_iter()
            .map(|mismatch| mismatch.field)
            .collect::<Vec<_>>();
        assert_eq!(
            fields,
            ["core_compatibility_id", "rom_sha256", "behavior_sha256"]
        );
    }
}
