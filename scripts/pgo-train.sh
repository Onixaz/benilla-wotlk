#!/usr/bin/env bash
# Records the PGO profile `cargo play` builds with: the player build exactly as it builds it,
# instrumented, one play session, and the merged profile at
# <primary checkout>/benilla-config/pgo/benilla-<thin|fat>.profdata, which replaces the last one of
# that mode. The profile is never committed.
#
# The client opens with `cargo play`'s settings: log in, play a few minutes through busy places (a
# city, a crowd, a fight), then quit the game normally, which writes the counters; a killed client
# writes none. Needs WOW_DATA, a server (WOW_HOST) and the `llvm-tools` rustup component. Record
# again after a toolchain bump, whose LLVM reads only its own profile format, and as the code
# drifts from the one the profile measured.
#
#   scripts/pgo-train.sh                      # thin LTO, as `cargo play`
#   BENILLA_PGO_LTO=fat scripts/pgo-train.sh  # fat LTO, as `BENILLA_PGO_LTO=fat cargo play`
set -euo pipefail

case "${BENILLA_PGO_LTO:-thin}" in
    thin) mode=thin cargo_profile=play ;;
    fat) mode=fat cargo_profile=release ;;
    *) echo "pgo-train: BENILLA_PGO_LTO=$BENILLA_PGO_LTO: \`thin\` or \`fat\`"; exit 1 ;;
esac

root="$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ] || [ ! -f "$root/scripts/pgo-train.sh" ]; then
    root="$(cd "$(dirname "$0")/.." && pwd)"
fi
cd "$root"

host="$(rustc -vV | sed -n 's/^host: //p')"
profdata="$(rustc --print sysroot)/lib/rustlib/$host/bin/llvm-profdata"
[ -x "$profdata" ] || [ -x "$profdata.exe" ] || {
    echo "pgo-train: llvm-profdata not found — rustup component add llvm-tools"
    exit 1
}

# The profile lives with the primary checkout's state, shared by every worktree.
primary="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")"
out="$primary/benilla-config/pgo"
file="$out/benilla-$mode.profdata"
mkdir -p "$out"

# The profile's function names hash each crate's cargo profile, features and target, so the
# training build is the optimized build's own (that cargo profile, no `dev`, the host target) in a
# folder of its own, which never invalidates it. The explicit target keeps the flag off build
# scripts and proc macros; the encoded form carries a path with spaces.
train="$root/target/pgo-train"
raw="$train/raw"
rm -rf "$raw"
mkdir -p "$raw"
CARGO_TARGET_DIR="$train" CARGO_ENCODED_RUSTFLAGS="-Cprofile-generate=$raw" \
    cargo build --profile "$cargo_profile" -p benilla --no-default-features --target "$host"

echo "pgo-train: play a few minutes through busy places, then quit the game normally"
BENILLA_HOME="${BENILLA_HOME:-$root/target/play/benilla-config}" \
    "$train/$host/$cargo_profile/benilla" || true

shopt -s nullglob
profiles=("$raw"/*.profraw)
[ "${#profiles[@]}" -gt 0 ] || {
    echo "pgo-train: the client wrote no profile; quit it from the game, not by killing it"
    exit 1
}
"$profdata" merge --sparse -o "$file.tmp" "${profiles[@]}"
mv "$file.tmp" "$file"
# The commit the profile measured, which `scripts/release/upload-profile.sh` labels it with.
git rev-parse --short HEAD >"$file.commit"
echo "pgo-train: $file — cargo play builds with it"
