# Architecture proposal

## Goal

Run the authentic Neo Geo games on Ubuntu and Windows through one maintained,
portable stack while preserving and diagnosing every locally owned byte.

## Runtime

Fork FinalBurn Neo at a pinned commit and add narrowly scoped legacy drivers for
the verified NeoRAGEx decrypted sets. Build native Linux SDL2 and Windows x86-64
artifacts from the same source. Keep MAME as an independent CRC oracle.

The launcher provides only strict commands:

- `metal-slug-classics doctor`
- `metal-slug-classics play <1|2|x|3|4|5>`
- `metal-slug-classics config`

The primary player runtime is the custom FBNeo **libretro core** inside a
minimal pinned RetroArch bundle. That reuses mature rollback/netplay, shaders,
input, save-state and replay infrastructure instead of recreating them. The
standalone SDL2 build remains the diagnostic fallback.

## LAN contract

The launcher exposes local host/discover/join over the same subnet and configures
RetroArch netplay against the custom FBNeo core. Internet matchmaking, public
relay and accounts are disabled. Each peer verifies the exact core, game, BIOS,
ROM-manifest and configuration hashes before starting; mismatches fail loudly.

The gate covers two physical Ubuntu/Windows PCs, latency/jitter display,
disconnect/reconnect, two-player completion and deterministic replay capture.

## ROM pipeline

1. Hash immutable source archives.
2. Match every member against known parent, decrypted and historical layouts.
3. Produce normalized derived ZIPs under `work/roms`; never edit sources.
4. Verify derived sets against the custom FBNeo driver and MAME where
   applicable.
5. Refuse launch when BIOS or payload integrity is unresolved.

The current Metal Slug X C3 member cannot be repaired from its corrupt bytes. A
valid locally owned source is required for that one member. The legacy BIOS data
is likewise unusable and must be reconstructed from a lawful verified source.

## Customization

- region and MVS/AES/UniBIOS mode;
- difficulty, lives, continues and blood through native DIP switches;
- per-game CPU clock profile, including an optional authentic or reduced
  Metal Slug 2 slowdown profile;
- integer scaling, shader, controller, latency and autosave profiles;
- save states, replay inputs and optional local cheats/patches with explicit
  hashes.

## What not to promise

- A generic ROM repairer able to synthesize missing/corrupt copyrighted bytes.
- Perfect behavior from arbitrary historical hacks.
- Cross-version save-state compatibility; replay inputs are the durable format.
