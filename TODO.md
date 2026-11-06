# TODO

- [x] Implement typed Rust manifests, metadata-only ZIP auditing, and
  deterministic derived-manifest plans (no payload synthesis).
- [x] Materialize complete rename-only plans with payload CRC32/size
  verification, deterministic driver ZIPs, atomic staged publication, and JSON
  provenance while preserving source archives.
- Add verified legacy drivers to the FBNeo fork.
- Produce native Linux and Windows runtime bundles.
- Validate a lawful BIOS source and replace the corrupt Metal Slug X C3 member.
- [ ] Add portable DIP switches and input replays. Portable profiles, governed
  controls, and isolated save/state paths are implemented.
- [x] Add fully editable keyboard/gamepad mappings with per-player and
  per-device profiles, axes/deadzones, hot remap, physical conflict detection,
  bounded portable import/export, backend limitation errors, and netplay
  behavioral hashing; defaults remain optional presets only.
- [ ] Build and pin the custom FBNeo libretro core plus a minimal governed
  RetroArch runtime (launcher contract is implemented).
- [x] Add zero-service same-subnet UDP broadcast/multicast discovery and strict
  RetroArch/FBNeo host/join orchestration with stale filtering, configurable
  interfaces/ports, and reconnect session metadata.
- [ ] Add latency diagnostics, automated live disconnect/reconnect coverage,
  full-game completion, and two-physical-PC Linux/Windows validation. Current
  tests cover packet/reconnect metadata and exact prelaunch contracts/arguments,
  not a real two-machine RetroArch session.
- Cross-pollinate authentic/free presets, level selection, profiles, snapshots, replays, and controller contracts with KOF Wing and Blur.
- [x] Add versioned per-profile language selection, strict EN/FR launcher
  catalogues, bounded atomic language import/export, and language/region
  netplay agreement.
- [x] Bind authentic Neo Geo region/system choices to the pinned FBNeo libretro
  `fbneo-neogeo-mode` core option and reject unsupported standalone overrides.
- [x] Add deterministic reversible translation-pack manifests generated and
  applied only under `work/`, with 100% coverage, exact base/result SHA-256,
  no-clobber atomic publication, and apply/revert tests.
- [ ] Produce and independently review a lawful French in-game pack for each
  title. Until every title reaches verified 100% coverage, French in-game
  support remains infrastructure only and must not be advertised as complete.
- [ ] Validate generated core-options files against physical Linux/Windows
  RetroArch builds and every available authentic MVS/AES region BIOS.
