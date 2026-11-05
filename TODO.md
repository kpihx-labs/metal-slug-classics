# TODO

- [x] Implement typed Rust manifests, metadata-only ZIP auditing, and
  deterministic derived-manifest plans (no payload synthesis).
- Add verified legacy drivers to the FBNeo fork.
- Produce native Linux and Windows runtime bundles.
- Validate a lawful BIOS source and replace the corrupt Metal Slug X C3 member.
- [ ] Add portable controls, DIP switches, saves, and input replays (portable
  profile storage and isolated save/state paths are implemented).
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
