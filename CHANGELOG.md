# Changelog

## 0.1.0 - Unreleased

- Establish immutable ROM auditing and cross-platform FBNeo architecture.
- Classify all six local Metal Slug archives by exact CRC family.
- Pin Linux and Windows FBNeo builds and MAME 0.285 audit tooling.
- Add project-local checks and modernization research.
- Add the Rust 2024 auditor/launcher with clear `cli`, `audit`, `controls`,
  `manifest`, `netplay`, `normalize`, `profile`, and `runtime` modules.
- Inspect ZIP member names, sizes, and CRC32 values without extraction and emit
  human or JSON classification reports.
- Encode modern and documented local legacy/decrypted manifests for all six
  games, including explicit corrupt and missing-BIOS diagnostics.
- Add deterministic blocked/ready derived-manifest plans without synthesizing
  ROM data.
- Separate safe ROM normalization from BIOS availability: complete sets carry
  `bios_required` provenance while launch remains blocked without a valid BIOS.
- Add explicit `--skip-blocked` collection normalization with payload checksum
  preflight and stable reporting for every omitted archive.
- Add native profile paths, isolated standalone FBNeo working/config paths, and
  exact audited-BIOS binding for standalone and libretro/RetroArch runtimes.
- Add canonical typed-manifest and behavioral-profile hashes, cross-platform
  runtime/core compatibility IDs, enforced local binary SHA-256 pins, and strict
  LAN agreement that excludes machine paths and host/client locator fields.
- Add same-subnet UDP broadcast/multicast LAN discovery, exact RetroArch
  `--host --port` and `--connect ADDRESS --port PORT` orchestration, bounded
  stale-session filtering, reconnect session metadata, multi-interface
  selection tied to the active route or explicit interfaces, and
  private-address defaults with explicit public-interface opt-in.
- Add stable JSON failure envelopes and strict pinned archive/executable doctor
  gates.
- Pin the maintained libretro FBNeo port separately from the standalone engine
  so RetroArch netplay always loads a real, governed core.
- Add synthetic ZIP, CRC metadata, profile path, runtime, and netplay tests plus
  end-to-end opposite-role/cross-path agreement and BIOS command tests.
- Select automatic LAN discovery only from the active default-route IPv4
  interface, reject tunnels and inactive bridge fallbacks, and use reported
  prefixes, netmasks, and directed broadcast addresses.
- Add `normalize` for explicit ZIP or directory sources: reuse audited manifest
  plans, reject missing BIOS/incomplete/corrupt/unknown or unsafe inputs,
  re-verify every payload CRC32 and size, and publish deterministic rename-only
  driver archives outside the immutable source.
- Add staged output publication with default no-clobber and explicit `--force`,
  rollback replacement, file/directory synchronization where supported, and
  path-independent JSON provenance containing source/output SHA-256 and
  complete source-to-manifest member mappings.
- Add unit and CLI integration coverage for source immutability, deterministic
  ZIP bytes/order/timestamps, provenance, no-clobber/force, incomplete/corrupt/
  unknown/missing-BIOS rejection, and traversal/symlink safety.
- Add a typed/versioned controls schema with per-player/per-device keyboard and
  gamepad actions, axes, deadzones, frontend hotkeys, conflict validation, and
  optional explicit `authentic`/`modern` presets.
- Add `controls show/set/reset/import/export/validate`, incremental hot remap
  and clear operations, bounded imports, native cross-platform profile paths,
  and same-directory synchronized profile/import/export replacement.
- Generate exact RetroArch input/hotkey settings and Linux/macOS standalone
  FBNeo per-game input records; reject backend limitations (including
  standalone Windows isolation) instead of silently ignoring unsupported
  hotkeys, deadzones, device multiplicity, or keyboard codes.
- Include canonical controls without machine-local logical device labels in the
  netplay behavioral hash and add unit, simulated runtime, hash, physical
  conflict, replacement round-trip, and end-to-end CLI coverage. Physical
  two-machine play/reconnect/completion validation remains a release task.
- Add a versioned per-profile language contract and `language
  list/show/set/validate/import/export` with complete, key-parity English and
  French launcher catalogues, strict one-step English fallback, bounded imports,
  and synchronized same-directory replacement.
- Add `language pack generate/validate/apply/revert` for deterministic,
  reversible byte-manifest patches derived exclusively from operator-owned
  files under `work/`; require 100% operator-declared coverage, equal sizes,
  exact base and result SHA-256, bounded non-overlapping reversible hunks, no
  complete literal payload, no symlinks, no out-of-work paths, no clobbering,
  and atomic publication.
- Include language and region explicitly in netplay protocol v3 and in the
  behavioral profile hash so a mismatch blocks launch before peer connection.
- Generate an isolated RetroArch core-options file and set the pinned FBNeo
  `fbneo-neogeo-mode` to authentic MVS/AES Europe/USA/Japan values selected by
  the profile; reject missing requested BIOS modes before the core can fall back
  to `DIPSWITCH`, and reject standalone region/system overrides that cannot be
  proven isolated.
- Add unit and CLI integration tests for catalogue coverage/fallback, profile
  migration, language import atomicity, language/region hashing, exact core
  options, deterministic reversible packs, wrong-base/incomplete/out-of-work
  rejection, no partial outputs, and native Linux/Windows/macOS profile paths.
- Refuse AES USA instead of silently substituting AES Europe: the pinned FBNeo
  core exposes USA only for MVS, while AES exposes Europe/Asia and Japan.
- Bound translation inputs, manifest bytes, hunk size/count, and cumulative
  patch bytes; reject traversal before creating output parents and test
  symlink, complete-payload, oversized-input, wrong-hash, apply, and revert
  failures without partial publication.
