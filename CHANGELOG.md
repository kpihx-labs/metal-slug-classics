# Changelog

## 0.1.0 - Unreleased

- Establish immutable ROM auditing and cross-platform FBNeo architecture.
- Classify all six local Metal Slug archives by exact CRC family.
- Pin Linux and Windows FBNeo builds and MAME 0.285 audit tooling.
- Add project-local checks and modernization research.
- Add the Rust 2024 auditor/launcher with clear `cli`, `audit`, `manifest`,
  `profile`, `runtime`, and `netplay` modules.
- Inspect ZIP member names, sizes, and CRC32 values without extraction and emit
  human or JSON classification reports.
- Encode modern and documented local legacy/decrypted manifests for all six
  games, including explicit corrupt and missing-BIOS diagnostics.
- Add deterministic blocked/ready derived-manifest plans without synthesizing
  ROM data.
- Add native profile paths, isolated standalone FBNeo working/config paths, and
  exact audited-BIOS binding for standalone and libretro/RetroArch runtimes.
- Add canonical typed-manifest and behavioral-profile hashes, cross-platform
  runtime/core compatibility IDs, enforced local binary SHA-256 pins, and strict
  LAN agreement that excludes machine paths and host/client locator fields.
- Add stable JSON failure envelopes and strict pinned archive/executable doctor
  gates.
- Add synthetic ZIP, CRC metadata, profile path, runtime, and netplay tests plus
  end-to-end opposite-role/cross-path agreement and BIOS command tests.
