#!/usr/bin/env bash
# Packages the player build the release workflow left in `target/<target>/release/` (fat LTO, PGO,
# as `BENILLA_PGO_LTO=fat cargo play` builds it) into the release archive:
# `dist/benilla-v<version>-<target>.zip` on Windows, `.tar.gz` elsewhere, holding the binary,
# benilla's two licenses and THIRD-PARTY-LICENSES, the notices the MIT, BSD, Zlib, MPL-2.0, OFL and
# Lua licenses of the code inside the binary require beside it. Needs `cargo-about` (`cargo install
# --locked cargo-about --features cli`).
#
#   scripts/release/package.sh <target-triple>
set -euo pipefail

target=${1:?usage: package.sh <target-triple>}
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)
[[ -n $version ]] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }

case $target in
    *windows*) exe=benilla.exe ;;
    *) exe=benilla ;;
esac
bin=target/$target/release/$exe
[[ -f $bin ]] || { echo "$bin missing: build it first" >&2; exit 1; }

name=benilla-v$version-$target
stage=dist/$name
rm -rf "$stage"
mkdir -p "$stage"

cp "$bin" LICENSE-MIT LICENSE-APACHE README.md "$stage/"

# The player build's graph, build dependencies included: `lua-src` is one, and compiles Lua into
# the binary.
cargo about generate --locked -c scripts/release/about.toml -m crates/benilla/Cargo.toml \
    --no-default-features scripts/release/about.hbs -o "$stage/THIRD-PARTY-LICENSES.txt"

# Lua's own notice, which `lua-src`'s license file (its crate author's) does not carry; upstream
# keeps it at the end of `lua.h`.
{
    printf '================================================================================\n'
    printf 'Lua 5.1.5 (MIT), compiled in through lua-src\n\n'
    sed -n '/^\* Copyright (C) .* Lua.org, PUC-Rio/,/^\* SOFTWARE OR THE USE/p' \
        third_party/lua-src/lua-5.1.5/lua.h | sed 's/^\* \{0,1\}//'
} >>"$stage/THIRD-PARTY-LICENSES.txt"
grep -q 'Lua.org, PUC-Rio' "$stage/THIRD-PARTY-LICENSES.txt" || { echo "Lua notice not found in lua.h" >&2; exit 1; }

cd dist
case $target in
    *windows*) archive=$name.zip; rm -f "$archive"; 7z a -tzip -bso0 "$archive" "$name" ;;
    *) archive=$name.tar.gz; tar -czf "$archive" "$name" ;;
esac
echo "dist/$archive"
