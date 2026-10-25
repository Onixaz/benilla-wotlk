#!/usr/bin/env bash
# Uploads this machine's fat-LTO PGO profile, the one `BENILLA_PGO_LTO=fat scripts/pgo-train.sh`
# records, as the release workflow's build input: asset `benilla-fat-<host>-rustc<version>.profdata`
# on the `pgo-profile` pre-release, replacing the last one for that target and toolchain. The
# workflow builds each target's release binary with its own, so record and upload on each
# platform; again after a toolchain bump, whose LLVM reads only its own profile format, and as the
# code drifts from the commit the profile measured. Needs `gh`, logged in with push access.
#
#   scripts/release/upload-profile.sh
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

host=$(rustc -vV | sed -n 's/^host: //p')
rustc_release=$(rustc -vV | sed -n 's/^release: //p')
primary=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
file=$primary/benilla-config/pgo/benilla-fat.profdata
[[ -f $file ]] || { echo "no $file: record it with BENILLA_PGO_LTO=fat scripts/pgo-train.sh" >&2; exit 1; }
commit=$(cat "$file.commit" 2>/dev/null || echo "an unknown commit")

tag=pgo-profile
if ! gh release view "$tag" >/dev/null 2>&1; then
    gh release create "$tag" --prerelease --target main --title "PGO profiles" --notes \
        "The release workflow's build input, not a release: each target's PGO profile, recorded with \`BENILLA_PGO_LTO=fat scripts/pgo-train.sh\` and uploaded with \`scripts/release/upload-profile.sh\`."
fi

asset=benilla-fat-$host-rustc$rustc_release.profdata
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "$file" "$stage/$asset"
gh release upload "$tag" "$stage/$asset#recorded at $commit, rustc $rustc_release" --clobber
echo "uploaded $asset, recorded at $commit"
