#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/vendor/fbneo-libretro"
OUTPUT="$ROOT/work/cores/linux-x86_64"
JOBS="${JOBS:-$(nproc)}"

[[ -f "$SOURCE/src/burner/libretro/Makefile" ]] || {
    printf 'FBNeo libretro submodule is missing; run git submodule update --init --recursive\n' >&2
    exit 1
}

make -j"$JOBS" -C "$SOURCE/src/burner/libretro"
mkdir -p "$OUTPUT"
install -m 0755 "$SOURCE/src/burner/libretro/fbneo_libretro.so" \
    "$OUTPUT/fbneo_libretro.so"
sha256sum "$OUTPUT/fbneo_libretro.so" | tee "$OUTPUT/fbneo_libretro.so.sha256"
printf 'FBNeo libretro commit: %s\n' "$(git -C "$SOURCE" rev-parse HEAD)"
