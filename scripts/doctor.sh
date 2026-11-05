#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

check_hash() {
    local expected="$1" file="$2" actual
    actual="$(sha256sum "$file" | cut -d' ' -f1)"
    [[ "$actual" == "$expected" ]] || {
        printf 'hash mismatch: %s\n' "$file" >&2
        return 1
    }
}

while IFS= read -r tracked; do
    case "${tracked,,}" in
        *.zip | *.7z | *.rom | *.bin)
            printf 'proprietary payload-like file is tracked: %s\n' "$tracked" >&2
            exit 1
            ;;
    esac
done < <(git -C "$ROOT" ls-files)

expected_submodule="$(git -C "$ROOT" rev-parse HEAD:vendor/fbneo)"
actual_submodule="$(git -C "$ROOT/vendor/fbneo" rev-parse HEAD)"
[[ "$actual_submodule" == "$expected_submodule" ]] || {
    printf 'FBNeo submodule mismatch: expected %s, found %s\n' \
        "$expected_submodule" "$actual_submodule" >&2
    exit 1
}
submodule_changes="$(git -C "$ROOT/vendor/fbneo" status --porcelain)"
[[ -z "$submodule_changes" ]] || {
    printf 'FBNeo submodule has local changes:\n%s\n' "$submodule_changes" >&2
    exit 1
}

check_hash fe2b00e2ca24dbf54cab37ae288b3cbc975881856301b807a31f25fe09a01d13 \
    "$ROOT/work/tools/linux-sdl2-x86_64.zip"
check_hash 7c58fd31c0e8d384af6938b62d1bb46b5c9ed38a779d66810b8c18bfbf6ea710 \
    "$ROOT/work/tools/windows-x86_64.zip"
[[ -x "$ROOT/work/tools/fbneo-linux/fbneo" ]] || {
    printf 'missing executable: %s\n' "$ROOT/work/tools/fbneo-linux/fbneo" >&2
    exit 1
}
[[ -x "$ROOT/work/tools/mame-root/usr/games/mame" ]] || {
    printf 'missing executable: %s\n' "$ROOT/work/tools/mame-root/usr/games/mame" >&2
    exit 1
}

printf 'metal-slug-classics repository/tools: ok (pins verified; no tracked payloads)\n'
