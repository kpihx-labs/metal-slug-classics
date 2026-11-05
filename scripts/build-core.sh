#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="$ROOT/vendor/fbneo-libretro"
JOBS="${JOBS:-$(nproc)}"
TARGET="${1:-linux}"

[[ -f "$SOURCE/src/burner/libretro/Makefile" ]] || {
    printf 'FBNeo libretro submodule is missing; run git submodule update --init --recursive\n' >&2
    exit 1
}

build_linux() {
    local output="$ROOT/work/cores/linux-x86_64"
    make clean -C "$SOURCE/src/burner/libretro"
    make -j"$JOBS" -C "$SOURCE/src/burner/libretro"
    mkdir -p "$output"
    install -m 0755 "$SOURCE/src/burner/libretro/fbneo_libretro.so" \
        "$output/fbneo_libretro.so"
    sha256sum "$output/fbneo_libretro.so" | tee "$output/fbneo_libretro.so.sha256"
}

build_windows() {
    local output="$ROOT/work/cores/windows-x86_64"
    make clean -C "$SOURCE/src/burner/libretro"
    make -j"$JOBS" -C "$SOURCE/src/burner/libretro" \
        platform=win \
        CC=x86_64-w64-mingw32-gcc \
        CXX=x86_64-w64-mingw32-g++ \
        AR=x86_64-w64-mingw32-ar \
        WINDRES=x86_64-w64-mingw32-windres
    mkdir -p "$output"
    install -m 0755 "$SOURCE/src/burner/libretro/fbneo_libretro.dll" \
        "$output/fbneo_libretro.dll"
    sha256sum "$output/fbneo_libretro.dll" | tee "$output/fbneo_libretro.dll.sha256"
}

case "$TARGET" in
    linux) build_linux ;;
    windows) build_windows ;;
    all)
        build_linux
        build_windows
        ;;
    *)
        printf 'usage: %s [linux|windows|all]\n' "$0" >&2
        exit 2
        ;;
esac

printf 'FBNeo libretro commit: %s\n' "$(git -C "$SOURCE" rev-parse HEAD)"
