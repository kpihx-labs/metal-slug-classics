# Metal Slug Classics

Cross-platform Rust 2024 auditor and governed launcher for **locally owned**
Neo Geo Metal Slug data. No ROM or BIOS payload is included, downloaded, or
silently synthesized.

The original archives remain immutable. Audits inspect ZIP central-directory
filename, uncompressed-size, and CRC32 metadata without extracting members.
The explicit `normalize` command then streams only a complete, recognized plan,
re-verifies every payload's size and CRC32, and writes canonical member names
to a separate output. The manifests cover the modern FBNeo layouts and the
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
metal-slug-classics normalize <ZIP_OR_DIRECTORY> --output <DIRECTORY> [--force]
metal-slug-classics play <1|2|x|3|4|5> [--profile NAME]
metal-slug-classics profile list
metal-slug-classics profile create NAME [OPTIONS]
metal-slug-classics controls show [PROFILE]
metal-slug-classics controls set [PROFILE] --player N --device ID --kind keyboard|gamepad --target ACTION --binding SPEC [--index N] [--deadzone PER_MILLE]
metal-slug-classics controls set [PROFILE] --player N --device ID --kind keyboard|gamepad --target ACTION --clear
metal-slug-classics controls reset [PROFILE] [--preset authentic|modern]
metal-slug-classics controls import INPUT.json [--profile PROFILE]
metal-slug-classics controls export OUTPUT.json [--profile PROFILE]
metal-slug-classics controls validate [PROFILE] [--input INPUT.json]
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
bytes can be repaired. For recognized layouts `audit` emits a deterministic,
hash-addressed **derived-manifest plan** describing canonical member names and
the required FBNeo driver. Auditing itself does not read member payloads.

`normalize` accepts one explicit ZIP or all recognized collection ZIPs directly
inside a directory. A valid sibling `neogeo.zip` remains required: the command
refuses `missing-bios`, `corrupt`, `unknown`, blocked, incomplete, duplicate, or
unsafe plans before publication. It performs rename-only materialization: every
source member is streamed byte-for-byte, with actual size and CRC32 checked
against the selected manifest, while extra members are omitted. Output is an
explicit directory outside the source location; the source is never modified.

Each archive is named from the selected manifest's real driver (for example
`mslug4nrx.zip`), stores members in manifest order under manifest names, and
uses a fixed ZIP timestamp, fixed permissions, and the deterministic `stored`
method. An adjacent `<driver>.zip.provenance.json` records the stable source
SHA-256 (not a machine-local path), manifest ID, every source-to-target name
with CRC32/size, and output SHA-256. Equal source bytes therefore produce
byte-identical provenance from different locations. The complete output
directory is staged next to its destination, files are synchronized, directory
metadata is synchronized where supported, and publication uses rename.
Existing output is refused unless `--force` is explicit. Forced replacement
uses a rollback backup and never exposes a partially written directory.
Portable standard-library rename cannot provide one atomic directory-exchange
primitive on every supported filesystem, especially Windows, so the
destination name can be briefly absent between the backup and publication
renames; rollback is attempted if publication fails.

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

1. **Standalone FBNeo**: isolated SDL FBNeo configuration and working directory
   on Linux/macOS, governed ROM/BIOS paths, joystick support, and autosave.
   The Windows frontend changes to the executable directory internally, so the
   launcher rejects standalone Windows rather than claim that its isolated
   generated files are consumed; use the libretro runtime there.
2. **Custom FBNeo libretro core + RetroArch**: explicit executable/core paths,
   isolated saves/states, the exact audited BIOS directory bound as RetroArch's
   `system_directory`, public netplay announcement/NAT traversal/MITM relay
   disabled, and direct LAN host/connect arguments.

The archive must have the canonical driver filename named in the audit plan
before launch; `normalize` can produce that verified archive without altering
payload bytes.

## Controls

Controls live inside each portable profile under a typed, versioned
`controls.schema_version` contract. New profiles contain no implicit mapping
unless `profile create --controls-preset authentic|modern` is requested.
`controls reset` likewise clears every mapping unless an explicit preset is
selected. A launch requires complete mappings for every declared player: up,
down, left, right, four fire buttons, start, and coin.

Mappings are per player and named device, with a keyboard/gamepad kind, physical
device index, per-mille deadzone, game actions, and frontend hotkeys. Bindings
use one of these CLI forms:

- `key:NAME` for RetroArch, or `key:NAME:0xCODE` when standalone FBNeo also
  needs its platform input code;
- `button:NUMBER`;
- `axis:NUMBER:+|−:THRESHOLD_PER_MILLE` (use the ASCII `+` or `-` in commands).

`controls set` supports hot remapping and `--clear`; incomplete edits are saved
as explicit drafts and reported as not launch-ready. `controls validate` checks
schema version, ranges, required actions, device identity, binding/device type,
deadzone/threshold coherence, and physical-input conflicts. Physical identity
is the device kind/index plus the actual key, button, or axis; logical IDs and
FBNeo metadata cannot hide a collision. Import is capped at 1 MiB and validates
before replacing a profile. Export and profile replacement use a same-directory
temporary file and synchronize it before replacement. A successful
same-filesystem replacement does not expose partially written JSON;
unsupported filesystems or sharing modes can reject the operation rather than
providing an absolute cross-platform atomicity guarantee.

RetroArch receives explicit `input_playerN_*`, joypad index, axis threshold, and
global hotkey settings in its isolated generated config. Because RetroArch has
one joypad index per player, one digital-axis threshold, and one binding per
global hotkey, configurations that cannot be represented exactly are rejected.
Keyboard and gamepad bindings may coexist for one player.

Standalone FBNeo receives a generated `config/games/<driver>.ini` in its
isolated working directory, using real `switch`, `joyaxis-neg`, and
`joyaxis-pos` records. Its per-game format cannot represent governed frontend
hotkeys or per-device deadzones, and named keyboard keys require an explicit
FBNeo numeric code; the launcher rejects these cases with actionable errors
instead of silently falling back or dropping settings. The optional presets use
portable RetroArch key names and therefore require explicit numeric key codes
before use with standalone FBNeo.

## LAN agreement

Every prepared launch verifies and records local runtime/core SHA-256 integrity,
then writes `session-contract.json` containing only peer-comparable behavioral
inputs: game, canonical typed-manifest content hash, runtime/core compatibility
IDs, ROM hash, the exact audited BIOS hash, and deterministic behavioral-profile
hash. Machine-local paths, logical device labels, native executable hashes,
host/client role, peer address, and port are deliberately excluded from peer
agreement. Physical device kind/index and every behavior-affecting binding
field remain included. This lets a Linux host and Windows client agree while
each still verifies its own binaries. The canonical behavioral controls
mappings are included in that hash, so any player, physical device, button,
axis, deadzone, keyboard code, or hotkey difference blocks netplay agreement
before launch.

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

`metal-slug-classics doctor` reports Rust-side configuration, profiles,
manifests, and an optional `METAL_SLUG_SOURCE` audit. The stricter
`scripts/doctor.sh`, run by `make check`, additionally requires the pinned
Linux/Windows runtime archives, their exact SHA-256 hashes, matching submodule
gitlinks, and the pinned local FBNeo/MAME executables. Missing research tool
artifacts are a failure, not an optional skip.

FinalBurn Neo standalone is pinned as `vendor/fbneo`; the separately maintained
libretro port is pinned as `vendor/fbneo-libretro`. The libretro build is the
primary player runtime; standalone SDL2 remains the diagnostic fallback. MAME
remains an independent preservation oracle.

Build the exact pinned Linux core with `make build-core`, the Windows core with
`make build-core-windows`, or both with `make build-cores`. Generated cores and
SHA-256 manifests live under `work/cores/<platform>/` and remain outside Git.
