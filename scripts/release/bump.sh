#!/usr/bin/env bash
# Bumps the workspace version for a release and commits it on the current branch. Landing that
# commit on main is the release: `.github/workflows/release.yml` sees a version with no release
# yet, builds `cargo ship` for Windows and Linux, and publishes it as release `v<version>`, tag
# included.
#
#   scripts/release/bump.sh <major|minor|patch|X.Y.Z>
set -euo pipefail

bump=${1:?usage: bump.sh <major|minor|patch|X.Y.Z>}
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

[[ $(git branch --show-current) != main ]] || { echo "on main: bump on a branch and land it by pull request" >&2; exit 1; }
git diff --quiet HEAD -- Cargo.toml Cargo.lock || { echo "Cargo.toml or Cargo.lock has uncommitted changes" >&2; exit 1; }

old=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)
IFS=. read -r major minor patch <<<"$old"
case $bump in
    major) new=$((major + 1)).0.0 ;;
    minor) new=$major.$((minor + 1)).0 ;;
    patch) new=$major.$minor.$((patch + 1)) ;;
    [0-9]*.[0-9]*.[0-9]*) new=$bump ;;
    *) echo "not major, minor, patch or X.Y.Z: $bump" >&2; exit 1 ;;
esac
[[ $new != "$old" ]] || { echo "already at $old" >&2; exit 1; }
if git ls-remote --exit-code --tags origin "refs/tags/v$new" >/dev/null 2>&1; then
    echo "tag v$new already exists on origin" >&2
    exit 1
fi

# The one `version = "…"` at the start of a line is `[workspace.package]`'s; every member inherits it.
sed -i.bak "s/^version = \"$old\"$/version = \"$new\"/" Cargo.toml && rm Cargo.toml.bak
cargo update --workspace --offline --quiet

git commit --quiet -m "Release v$new" -- Cargo.toml Cargo.lock
echo "v$old -> v$new, committed; land it by pull request and the release workflow tags and publishes v$new"
