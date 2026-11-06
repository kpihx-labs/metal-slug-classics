use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameId {
    One,
    Two,
    X,
    Three,
    Four,
    Five,
}

impl GameId {
    #[must_use]
    pub const fn driver(self) -> &'static str {
        match self {
            Self::One => "mslug",
            Self::Two => "mslug2",
            Self::X => "mslugx",
            Self::Three => "mslug3",
            Self::Four => "mslug4",
            Self::Five => "mslug5",
        }
    }
}

impl fmt::Display for GameId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::One => "1",
            Self::Two => "2",
            Self::X => "x",
            Self::Three => "3",
            Self::Four => "4",
            Self::Five => "5",
        })
    }
}

impl FromStr for GameId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "1" => Ok(Self::One),
            "2" => Ok(Self::Two),
            "x" => Ok(Self::X),
            "3" => Ok(Self::Three),
            "4" => Ok(Self::Four),
            "5" => Ok(Self::Five),
            _ => Err(format!(
                "unsupported game '{value}'; expected 1, 2, x, 3, 4, or 5"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutKind {
    ModernCompatible,
    DocumentedLegacyDecrypted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RomMember {
    pub name: &'static str,
    pub size: u64,
    pub crc32: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetManifest {
    pub id: &'static str,
    pub game: GameId,
    pub driver: &'static str,
    pub layout: LayoutKind,
    pub members: Vec<RomMember>,
}

impl SetManifest {
    /// Hashes the complete typed manifest in a canonical member order.
    ///
    /// # Errors
    /// Returns an error if the canonical manifest cannot be serialized.
    pub fn content_sha256(&self) -> Result<String, serde_json::Error> {
        let mut members = self.members.clone();
        members.sort_by_key(|member| (member.name, member.size, member.crc32));
        let canonical =
            serde_json::to_vec(&(1_u32, self.id, self.game, self.driver, self.layout, members))?;
        Ok(format!("{:x}", Sha256::digest(canonical)))
    }
}

const fn rom(name: &'static str, size: u64, crc32: u32) -> RomMember {
    RomMember { name, size, crc32 }
}

#[must_use]
pub fn manifests() -> Vec<SetManifest> {
    vec![
        SetManifest {
            id: "mslug-modern",
            game: GameId::One,
            driver: "mslug",
            layout: LayoutKind::ModernCompatible,
            members: vec![
                rom("201-p1.p1", 0x20_0000, 0x08d8_daa5),
                rom("201-s1.s1", 0x02_0000, 0x2f55_958d),
                rom("201-c1.c1", 0x40_0000, 0x7281_3676),
                rom("201-c2.c2", 0x40_0000, 0x96f6_2574),
                rom("201-c3.c3", 0x40_0000, 0x5121_456a),
                rom("201-c4.c4", 0x40_0000, 0xf4ad_59a3),
                rom("201-m1.m1", 0x02_0000, 0xc28b_3253),
                rom("201-v1.v1", 0x40_0000, 0x23d2_2ed1),
                rom("201-v2.v2", 0x40_0000, 0x472c_f9db),
            ],
        },
        SetManifest {
            id: "mslug-pre-mame-0.80",
            game: GameId::One,
            driver: "mslug-legacy",
            layout: LayoutKind::DocumentedLegacyDecrypted,
            members: vec![
                rom("201-p1.p1", 0x20_0000, 0x08d8_daa5),
                rom("201-s1.s1", 0x02_0000, 0x2f55_958d),
                rom("201-c1.c1", 0x40_0000, 0xd00b_d152),
                rom("201-c2.c2", 0x40_0000, 0xddff_1dea),
                rom("201-c3.c3", 0x40_0000, 0xd3d5_f9e5),
                rom("201-c4.c4", 0x40_0000, 0x5ac1_d497),
                rom("201-m1.m1", 0x02_0000, 0xc28b_3253),
                rom("201-v1.v1", 0x40_0000, 0x23d2_2ed1),
                rom("201-v2.v2", 0x40_0000, 0x472c_f9db),
            ],
        },
        SetManifest {
            id: "mslug2-modern",
            game: GameId::Two,
            driver: "mslug2",
            layout: LayoutKind::ModernCompatible,
            members: vec![
                rom("241-p1.p1", 0x10_0000, 0x2a53_c5da),
                rom("241-p2.sp2", 0x20_0000, 0x3888_3f44),
                rom("241-s1.s1", 0x02_0000, 0xf3d3_2f0f),
                rom("241-c1.c1", 0x80_0000, 0x394b_5e0d),
                rom("241-c2.c2", 0x80_0000, 0xe580_6221),
                rom("241-c3.c3", 0x80_0000, 0x9f6b_fa6f),
                rom("241-c4.c4", 0x80_0000, 0x7d3e_306f),
                rom("241-m1.m1", 0x02_0000, 0x9452_0ebd),
                rom("241-v1.v1", 0x40_0000, 0x99ec_20e8),
                rom("241-v2.v2", 0x40_0000, 0xecb1_6799),
            ],
        },
        SetManifest {
            id: "mslugx-modern",
            game: GameId::X,
            driver: "mslugx",
            layout: LayoutKind::ModernCompatible,
            members: vec![
                rom("250-p1.p1", 0x10_0000, 0x81f1_f60b),
                rom("250-p2.ep1", 0x40_0000, 0x1fda_2e12),
                rom("250-s1.s1", 0x02_0000, 0xfb6f_441d),
                rom("250-c1.c1", 0x80_0000, 0x09a5_2c6f),
                rom("250-c2.c2", 0x80_0000, 0x3167_9821),
                rom("250-c3.c3", 0x80_0000, 0xfd60_2019),
                rom("250-c4.c4", 0x80_0000, 0x3135_4513),
                rom("250-c5.c5", 0x80_0000, 0xa4b5_6124),
                rom("250-c6.c6", 0x80_0000, 0x83e3_e69d),
                rom("250-m1.m1", 0x02_0000, 0xfd42_a842),
                rom("250-v1.v1", 0x40_0000, 0xc79e_de73),
                rom("250-v2.v2", 0x40_0000, 0xea9a_abe1),
                rom("250-v3.v3", 0x20_0000, 0x2ca6_5102),
            ],
        },
        mslug3_modern(),
        mslug3_legacy(),
        mslug4_modern(),
        mslug4_legacy(),
        mslug4_neoragex(),
        mslug5_modern(),
        mslug5_legacy(),
    ]
}

fn mslug3_modern() -> SetManifest {
    SetManifest {
        id: "mslug3-modern",
        game: GameId::Three,
        driver: "mslug3",
        layout: LayoutKind::ModernCompatible,
        members: vec![
            rom("green.neo-sma", 0x04_0000, 0x9cd5_5736),
            rom("256-pg1.p1", 0x40_0000, 0xb07e_dfd5),
            rom("256-pg2.p2", 0x40_0000, 0x6097_c26b),
            rom("256-c1.c1", 0x80_0000, 0x5a79_c34e),
            rom("256-c2.c2", 0x80_0000, 0x944c_362c),
            rom("256-c3.c3", 0x80_0000, 0x6e69_d36f),
            rom("256-c4.c4", 0x80_0000, 0xb755_b4eb),
            rom("256-c5.c5", 0x80_0000, 0x7aac_ab47),
            rom("256-c6.c6", 0x80_0000, 0xc698_fd5d),
            rom("256-c7.c7", 0x80_0000, 0xcfce_ddd2),
            rom("256-c8.c8", 0x80_0000, 0x4d9b_e34c),
            rom("256-m1.m1", 0x08_0000, 0xeaee_c116),
            rom("256-v1.v1", 0x40_0000, 0xf269_0241),
            rom("256-v2.v2", 0x40_0000, 0x7e2a_10bd),
            rom("256-v3.v3", 0x40_0000, 0x0eae_c17c),
            rom("256-v4.v4", 0x40_0000, 0x9b4b_22d4),
        ],
    }
}

fn mslug3_legacy() -> SetManifest {
    SetManifest {
        id: "mslug3-neoragex-decrypted",
        game: GameId::Three,
        driver: "mslug3-legacy",
        layout: LayoutKind::DocumentedLegacyDecrypted,
        members: vec![
            rom("256-p1d.bin", 0x10_0000, 0xc871_c036),
            rom("256-p2d.bin", 0x40_0000, 0x1f3d_8ce8),
            rom("256-s1d.bin", 0x02_0000, 0x8458_fff9),
            rom("256-m1d.bin", 0x08_0000, 0xeaee_c116),
            rom("256-v1d.bin", 0x40_0000, 0xf269_0241),
            rom("256-v2d.bin", 0x40_0000, 0x7e2a_10bd),
            rom("256-v3d.bin", 0x40_0000, 0x0eae_c17c),
            rom("256-v4d.bin", 0x40_0000, 0x9b4b_22d4),
            rom("256-c1d.bin", 0x80_0000, 0x3540_398c),
            rom("256-c2d.bin", 0x80_0000, 0xbdd2_20f0),
            rom("256-c3d.bin", 0x80_0000, 0xbfaa_de82),
            rom("256-c4d.bin", 0x80_0000, 0x1463_add6),
            rom("256-c5d.bin", 0x80_0000, 0x48ca_7f28),
            rom("256-c6d.bin", 0x80_0000, 0x806e_b36f),
            rom("256-c7d.bin", 0x80_0000, 0x9395_b809),
            rom("256-c8d.bin", 0x80_0000, 0xa369_f9d4),
        ],
    }
}

fn mslug4_modern() -> SetManifest {
    SetManifest {
        id: "mslug4-modern",
        game: GameId::Four,
        driver: "mslug4",
        layout: LayoutKind::ModernCompatible,
        members: vec![
            rom("263-p1.p1", 0x10_0000, 0x27e4_def3),
            rom("263-p2.sp2", 0x40_0000, 0xfdb7_aed8),
            rom("263-c1.c1", 0x80_0000, 0x8486_5f8a),
            rom("263-c2.c2", 0x80_0000, 0x81df_97f2),
            rom("263-c3.c3", 0x80_0000, 0x1a34_3323),
            rom("263-c4.c4", 0x80_0000, 0x942c_fb44),
            rom("263-c5.c5", 0x80_0000, 0xa748_854f),
            rom("263-c6.c6", 0x80_0000, 0x5c8b_a116),
            rom("263-m1.m1", 0x02_0000, 0x46ac_8228),
            rom("263-v1.v1", 0x80_0000, 0x01e9_b9cd),
            rom("263-v2.v2", 0x80_0000, 0x4ab2_bf81),
        ],
    }
}

fn mslug4_legacy() -> SetManifest {
    SetManifest {
        id: "mslug4nd-decrypted-c",
        game: GameId::Four,
        driver: "mslug4-legacy",
        layout: LayoutKind::DocumentedLegacyDecrypted,
        members: vec![
            rom("263-p1.p1", 0x10_0000, 0x27e4_def3),
            rom("263-p2.sp2", 0x40_0000, 0xfdb7_aed8),
            rom("ms4n_c1.rom", 0x80_0000, 0xa75f_fcde),
            rom("ms4n_c2.rom", 0x80_0000, 0x5ab0_d12b),
            rom("ms4n_c3.rom", 0x80_0000, 0x61af_560c),
            rom("ms4n_c4.rom", 0x80_0000, 0xf2c5_44fd),
            rom("ms4n_c5.rom", 0x80_0000, 0x84c6_6c44),
            rom("ms4n_c6.rom", 0x80_0000, 0x5ed0_18ab),
            rom("263-m1.m1", 0x02_0000, 0x46ac_8228),
            rom("263-v1.v1", 0x80_0000, 0x01e9_b9cd),
            rom("263-v2.v2", 0x80_0000, 0x4ab2_bf81),
        ],
    }
}

fn mslug4_neoragex() -> SetManifest {
    SetManifest {
        id: "mslug4-neoragex-fully-decrypted",
        game: GameId::Four,
        driver: "mslug4-neoragex",
        layout: LayoutKind::DocumentedLegacyDecrypted,
        members: vec![
            rom("263-p1d.bin", 0x10_0000, 0x4d7e_6624),
            rom("263-p2d.bin", 0x40_0000, 0xfdb7_aed8),
            rom("263-s1d.bin", 0x02_0000, 0xa944_6774),
            rom("263-m1d.bin", 0x02_0000, 0xee1e_5580),
            rom("263-v1d.bin", 0x40_0000, 0xc1b2_af81),
            rom("263-v2d.bin", 0x40_0000, 0xcbbd_c6fa),
            rom("263-v3d.bin", 0x40_0000, 0xc4f1_a3cc),
            rom("263-v4d.bin", 0x40_0000, 0x4012_6a0a),
            rom("263-c1d.bin", 0x80_0000, 0x361d_dda6),
            rom("263-c2d.bin", 0x80_0000, 0x15d1_92c4),
            rom("263-c3d.bin", 0x80_0000, 0x63fb_79ff),
            rom("263-c4d.bin", 0x80_0000, 0xe862_3126),
            rom("263-c5d.bin", 0x40_0000, 0x3348_dc5d),
            rom("263-c6d.bin", 0x40_0000, 0xd90f_c1a0),
        ],
    }
}

fn mslug5_modern() -> SetManifest {
    SetManifest {
        id: "mslug5-modern",
        game: GameId::Five,
        driver: "mslug5",
        layout: LayoutKind::ModernCompatible,
        members: vec![
            rom("268-p1cr.p1", 0x40_0000, 0xd046_6792),
            rom("268-p2cr.p2", 0x40_0000, 0xfbf6_b61e),
            rom("268-c1c.c1", 0x80_0000, 0xab7c_389a),
            rom("268-c2c.c2", 0x80_0000, 0x3560_881b),
            rom("268-c3c.c3", 0x80_0000, 0x3af9_55ea),
            rom("268-c4c.c4", 0x80_0000, 0xc329_c373),
            rom("268-c5c.c5", 0x80_0000, 0x959c_8177),
            rom("268-c6c.c6", 0x80_0000, 0x010a_831b),
            rom("268-c7c.c7", 0x80_0000, 0x6d72_a969),
            rom("268-c8c.c8", 0x80_0000, 0x551d_720e),
            rom("268-m1.m1", 0x08_0000, 0x4a5a_6e0e),
            rom("268-v1c.v1", 0x80_0000, 0xae31_d60c),
            rom("268-v2c.v2", 0x80_0000, 0xc406_13ed),
        ],
    }
}

fn mslug5_legacy() -> SetManifest {
    SetManifest {
        id: "mslug5-neoragex-5.0",
        game: GameId::Five,
        driver: "mslug5-legacy",
        layout: LayoutKind::DocumentedLegacyDecrypted,
        members: vec![
            rom("268d-p1.rom", 0x10_0000, 0x5d1a_8668),
            rom("268d-p2.rom", 0x40_0000, 0x4766_2c5e),
            rom("268d-s1.rom", 0x02_0000, 0x6495_2683),
            rom("268d-m1.rom", 0x04_0000, 0x2f62_1a2b),
            rom("268d-v1.rom", 0x80_0000, 0xe443_be70),
            rom("268d-v2.rom", 0x80_0000, 0x8dc2_5965),
            rom("268d-c1.rom", 0x80_0000, 0x969c_0d62),
            rom("268d-c2.rom", 0x80_0000, 0xc69a_e867),
            rom("268d-c3.rom", 0x80_0000, 0xd7be_aeaf),
            rom("268d-c4.rom", 0x80_0000, 0x899f_b2af),
            rom("268d-c5.rom", 0x80_0000, 0x2fa1_a5ad),
            rom("268d-c6.rom", 0x80_0000, 0x6de8_9589),
            rom("268d-c7.rom", 0x80_0000, 0x97bd_0c0a),
            rom("268d-c8.rom", 0x80_0000, 0xc0d5_bc20),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_game_has_a_modern_and_local_layout() {
        let values = manifests();
        for game in [GameId::One, GameId::Three, GameId::Four, GameId::Five] {
            assert!(
                values
                    .iter()
                    .any(|item| item.game == game && item.layout == LayoutKind::ModernCompatible)
            );
            assert!(
                values.iter().any(|item| item.game == game
                    && item.layout == LayoutKind::DocumentedLegacyDecrypted)
            );
        }
        for game in [GameId::Two, GameId::X] {
            assert!(values.iter().any(|item| item.game == game));
        }
    }

    #[test]
    fn manifest_ids_and_member_signatures_are_well_formed() {
        let values = manifests();
        let mut ids = std::collections::BTreeSet::new();
        for manifest in values {
            assert!(ids.insert(manifest.id));
            assert_ne!(manifest.members, Vec::new());
            for member in manifest.members {
                assert_ne!(member.name, "");
                assert!(member.size > 0);
                assert_ne!(member.crc32, 0);
            }
        }
    }

    #[test]
    fn content_hash_is_order_independent_and_content_sensitive() {
        let original = manifests().remove(0);
        let mut reordered = original.clone();
        reordered.members.reverse();
        assert_eq!(
            original.content_sha256().unwrap(),
            reordered.content_sha256().unwrap()
        );
        reordered.members[0].crc32 ^= 1;
        assert_ne!(
            original.content_sha256().unwrap(),
            reordered.content_sha256().unwrap()
        );
    }
}
