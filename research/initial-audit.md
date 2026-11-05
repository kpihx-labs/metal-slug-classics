# Initial audit

## Local sets

- Metal Slug 2: complete set recognized by MAME 0.285.
- Metal Slug X: all entries match except `250-c3`, whose archive payload is
  corrupt (`bad CRC a9e1a296`, expected `fd602019`).
- Metal Slug 1: valid historical pre-MAME 0.80 graphics set; four C ROM CRCs
  differ from the modern parent set by design.
- Metal Slug 3: mixed NeoRAGEx decrypted set. Its eight decrypted C ROMs match
  FBNeo's documented decrypted components.
- Metal Slug 4: old fully decrypted NeoRAGEx/Kawaks-style set. Its CRC family is
  documented, but not accepted by the current MAME parent driver.
- Metal Slug 5: old decrypted/bootleg-compatible set. Its graphics, audio and
  text CRC family is documented; current MAME expects another parent layout.
- `neogeo.zip`: not a valid ZIP archive and cannot serve as a modern BIOS set.

## Runtime evidence

- MAME audit oracle: 0.285.
- FBNeo source commit: `9b081dcc1af570077702ce8a65278cc07959fcd8`.
- FBNeo Linux SDL2 archive SHA-256:
  `fe2b00e2ca24dbf54cab37ae288b3cbc975881856301b807a31f25fe09a01d13`.
- FBNeo Windows x86-64 archive SHA-256:
  `7c58fd31c0e8d384af6938b62d1bb46b5c9ed38a779d66810b8c18bfbf6ea710`.

The normalizer must never download ROMs. It may only rename, split, join,
transform and verify bytes already present in locally owned inputs.

## Legacy-set interpretation

- Metal Slug 1 graphics CRCs are the documented pre-MAME 0.80 set, not random
  corruption.
- Metal Slug 3 exactly matches a documented fully decrypted NeoRAGEx layout,
  including program CRC `c871c036` and all eight decrypted graphics ROMs.
- Metal Slug 4 exactly matches the documented `mslug4nd` family.
- Metal Slug 5 program CRCs `5d1a8668` and `47662c5e` identify the NeoRAGEx 5.0
  updated set; its decrypted graphics/audio family is likewise documented.
- Metal Slug X is the only game archive with proven payload corruption.
- The emulator executable, `zlib.dll`, UniBIOS archive and `neogeo.zip` in the
  legacy folder are opaque random-looking data rather than runnable PE/ZIP
  files. They cannot be trusted as runtime or BIOS inputs.

The practical route is a small FBNeo legacy-driver patch plus a verified BIOS,
not irreversible conversion to encrypted parent ROMs.
