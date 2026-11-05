#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE="${METAL_SLUG_SOURCE:-}"

check_hash() {
    local expected="$1" file="$2" actual
    actual="$(sha256sum "$file" | cut -d' ' -f1)"
    [[ "$actual" == "$expected" ]] || {
        printf 'hash mismatch: %s\n' "$file" >&2
        return 1
    }
}

check_hash fe2b00e2ca24dbf54cab37ae288b3cbc975881856301b807a31f25fe09a01d13 \
    "$ROOT/work/tools/linux-sdl2-x86_64.zip"
check_hash 7c58fd31c0e8d384af6938b62d1bb46b5c9ed38a779d66810b8c18bfbf6ea710 \
    "$ROOT/work/tools/windows-x86_64.zip"
[[ -x "$ROOT/work/tools/fbneo-linux/fbneo" ]]
[[ -x "$ROOT/work/tools/mame-root/usr/games/mame" ]]

if [[ -n "$SOURCE" ]]; then
    for game in 1 2 3 4 5 x; do
        [[ -f "$SOURCE/ROMs/metalslug${game}.zip" ]]
    done
fi

printf 'metal-slug-classics tools: ok\n'
