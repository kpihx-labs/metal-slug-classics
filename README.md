# Metal Slug Classics

Cross-platform Rust 2024 auditor and governed launcher for **locally owned**
Neo Geo Metal Slug data. No ROM or BIOS payload is included, downloaded, or
silently synthesized.

The original archives remain immutable. ZIPs are inspected through their
central-directory filename, uncompressed-size, and CRC32 metadata without
extracting members. The auditor recognizes the modern FBNeo layouts and the
documented historical/decrypted layouts found in the local collection for
Metal Slug 1, 2, X, 3, 4, and 5.

## Build and verify

Rust 1.85 or newer is required for the 2024 edition.

```sh
cargo build --release
make check
```

`make check` runs formatting, strict Clippy, all tests, a release build, shell
checks, and the project doctor. Test ZIPs contain only synthetic bytes.

## CLI

```text
metal-slug-classics doctor
metal-slug-classics audit [path]
metal-slug-classics play <1|2|x|3|4|5> [--profile NAME]
metal-slug-classics profile list
metal-slug-classics profile create NAME [OPTIONS]
metal-slug-classics lan doctor [LAN DISCOVERY OPTIONS]
metal-slug-classics lan discover [LAN DISCOVERY OPTIONS] [--timeout-ms MS]
metal-slug-classics lan host <1|2|x|3|4|5> [--profile NAME] [LAN DISCOVERY OPTIONS] [--game-port PORT] [--advertisement-ttl-ms MS] [--reconnect-seconds SEC]
metal-slug-classics lan join <1|2|x|3|4|5> [--profile NAME] [LAN DISCOVERY OPTIONS] [--timeout-ms MS] [--session-id ID]
```

`LAN DISCOVERY OPTIONS` are `--interface ADDRESS` (repeatable),
`--discovery-port PORT`, `--multicast-address ADDRESS`, and
`--allow-public-lan`.

Add `--json` to any command for machine-readable output. Failures use the
stable envelope `{"ok":false,"error":{"code":"…","message":"…"}}` and a
non-zero exit status. `audit` accepts one ZIP or a directory of ZIPs. Its
classifications are:

- `modern-compatible`;
- `documented-legacy-decrypted`;
- `corrupt` (also used for identified but incomplete sets);
- `missing-bios`;
- `unknown`.

Corrupt members are reported as corrupt. The tool never claims that damaged
bytes can be repaired. For recognized layouts it emits a deterministic,
hash-addressed **derived-manifest plan** describing canonical member names and
the required FBNeo driver. The plan does not read member payloads or write a
derived ROM archive.

## Profiles and runtimes

Profiles are JSON files under the native per-user configuration directory:

- Linux: `$XDG_CONFIG_HOME/metal-slug-classics` or
  `$HOME/.config/metal-slug-classics`;
- Windows: `%APPDATA%\MetalSlugClassics`;
- macOS: `$HOME/Library/Application Support/MetalSlugClassics`.

`profile create` never overwrites an existing profile. It requires an explicit
`--runtime-compatibility-id` and local `--runtime-sha256` pin; libretro profiles
also require `--core-compatibility-id` and `--core-sha256`. Compatibility IDs
identify behaviorally compatible builds across operating systems without
pretending that Linux and Windows native binaries have the same hash. The local
pins are checked before every launch. A profile selects one of two runtime
contracts:

1. **Standalone FBNeo**: isolated FBNeo configuration and working directory
   (including Windows), governed ROM/BIOS paths, joystick support, and autosave.
2. **Custom FBNeo libretro core + RetroArch**: explicit executable/core paths,
   isolated saves/states, the exact audited BIOS directory bound as RetroArch's
   `system_directory`, public netplay announcement/NAT traversal/MITM relay
   disabled, and direct LAN host/connect arguments.

The archive must already have the canonical driver filename named in the audit
plan before launch. This project does not normalize payload bytes itself.

## LAN agreement

Every prepared launch verifies and records local runtime/core SHA-256 integrity,
then writes `session-contract.json` containing only peer-comparable behavioral
inputs: game, canonical typed-manifest content hash, runtime/core compatibility
IDs, ROM hash, the exact audited BIOS hash, and deterministic behavioral-profile
hash. Machine-local paths, native executable hashes, host/client role, peer
address, and port are deliberately excluded from peer agreement. This lets a
Linux host and Windows client agree while each still verifies its own binaries.

The `lan` commands need no service, cloud account, public announcement, VPN,
port forwarding, or firewall mutation. A host advertises a bounded JSON packet
over same-subnet UDP broadcast and administratively scoped multicast (TTL 1).
Discovery listens briefly, ignores malformed and stale packets, and accepts
only source addresses that match a subnet represented by the selected local
interfaces, while preserving explicit loopback selection. Automatic local
interface selection uses only the active default-route IPv4 address and requires
a real broadcast address and a prefix shorter than `/32`; it never falls back to
inactive private bridges. A tunnel default route fails with an actionable error.
Repeat `--interface` to select exact local broadcast-capable IPv4 addresses, or
loopback for diagnostics. `--discovery-port`, `--multicast-address`, and
`--game-port` override the defaults. Public-address interface use requires the
explicit `--allow-public-lan` opt-in. Directed broadcasts and source-subnet
checks use the interface's reported broadcast address and netmask.

`lan host` prepares an exact RetroArch launch with `--host --port` before the
single content archive, advertises its
behavioral contract while RetroArch runs, and includes a stable session ID plus
timestamps for reconnect selection. `lan join` can use `--session-id` to select
that session, rebuilds and verifies its own local contract, rejects any game,
manifest/profile, ROM, BIOS, runtime, or core mismatch, then launches with exact
`--connect ADDRESS --port PORT` arguments before the single content archive.
Advertisements contain no ROM/BIOS, filesystem paths, executable paths, or
native binary hashes. Use `--dry-run` to
inspect a prepared launch without advertising or starting RetroArch.

The lower-level governed `play` path remains strict: when a profile enables
netplay and requires a peer contract, launch is blocked unless the contract in
`METAL_SLUG_PEER_CONTRACT` agrees on every behavioral field. `lan doctor` can
also compare `METAL_SLUG_LOCAL_CONTRACT` and `METAL_SLUG_PEER_CONTRACT` while
checking usable local interfaces.

The project doctor also requires the pinned Linux/Windows runtime archives,
their exact SHA-256 hashes, and the pinned local FBNeo/MAME executables. Missing
research tool artifacts are a failure, not an optional skip.

FinalBurn Neo standalone is pinned as `vendor/fbneo`; the separately maintained
libretro port is pinned as `vendor/fbneo-libretro`. The libretro build is the
primary player runtime; standalone SDL2 remains the diagnostic fallback. MAME
remains an independent preservation oracle.

Build the exact pinned Linux core with `make build-core`, the Windows core with
`make build-core-windows`, or both with `make build-cores`. Generated cores and
SHA-256 manifests live under `work/cores/<platform>/` and remain outside Git.
