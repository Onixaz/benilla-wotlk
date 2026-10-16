#!/usr/bin/env bash
# The in-loop verify, with the client data: fmt, clippy and test on the whole workspace, the player
# tests, and the engine boot against the install.
#
# The change set is the fork point against main plus staged, unstaged and untracked files: none
# verifies nothing, and docs/ and *.md alone run fmt only. CHECK_SCOPED=1 narrows a round to the
# changed crates and their dependents (a path under crates/<dir>/ scopes that package, any other
# path the whole workspace), the player tests to when `benilla-app` or `benilla-formats` is in
# scope, and the engine boot to when `benilla-worldview` is. CHECK_FULL=1 checks the whole
# workspace even with no changes.
# Where the install (WOW_DATA, or the resolver's own ladder) and the addon corpus
# (BENILLA_ADDON_CORPUS, or the `wow-addons-vanilla` link) both resolve, BENILLA_REQUIRE_DATA=1
# turns a data-gated skip into a failure. The land gates, scripts/gates.sh, read no data at all.
#
#   scripts/check.sh
set -uo pipefail

# Check the checkout you stand in when it is one of this repo, else this file's (as gates.sh does).
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[ -n "$root" ] && [ -f "$root/scripts/check.sh" ] || root="$here"
cd "$root" || exit 1

# The first Python 3 that runs: `python3` is a Microsoft Store stub on some Windows machines.
python=""
for p in python3 "py -3" python; do
    if $p -c 'import sys; sys.exit(sys.version_info < (3, 8))' >/dev/null 2>&1; then
        python="$p"
        break
    fi
done

# ── The round's change set ───────────────────────────────────────────────────────────────────────
base="$(git merge-base HEAD main 2>/dev/null || git rev-parse HEAD)"
changed="$( { git diff --name-only "$base" HEAD 2>/dev/null
              git diff --name-only HEAD 2>/dev/null
              git ls-files --others --exclude-standard 2>/dev/null; } | sort -u )"

if [ -z "$changed" ] && [ "${CHECK_FULL:-}" != "1" ]; then
    echo "check: no changes against main and a clean tree — nothing to verify"
    exit 0
fi

# ── Map files → crate dirs; anything unmappable scopes the whole workspace ───────────────────────
dirs=""
full_reason=""
[ "${CHECK_FULL:-}" = "1" ] && full_reason="CHECK_FULL=1"
while IFS= read -r f; do
    case "$f" in
    "") ;;
    docs/* | *.md) ;;                                 # provably gate-inert (gates.sh's docs-only rule)
    crates/*/*) d="${f#crates/}"; dirs="$dirs DIR:crates/${d%%/*}" ;;
    *) [ -n "$full_reason" ] || full_reason="'$f' is outside the crate map" ;;
    esac
done <<EOF
$changed
EOF

dirlist="$(printf '%s\n' $dirs | sed 's/.*DIR://' | sort -u)"
if [ -z "$full_reason" ] && [ -n "$dirlist" ] && [ "${CHECK_SCOPED:-}" != "1" ]; then
    full_reason="the default; CHECK_SCOPED=1 narrows to the changed crates"
fi
if [ -z "$full_reason" ] && [ -z "$dirlist" ]; then
    echo "check: docs-only round ($(printf '%s\n' "$changed" | wc -l | tr -d ' ') file(s)) — no gate can change; fmt only"
    cargo fmt --all -- --check || exit 1
    echo "check: green (docs only)"
    exit 0
fi

# ── Changed crates → reverse-dependency closure, off cargo metadata ──────────────────────────────
if [ -n "$full_reason" ]; then
    echo "check: the whole workspace ($full_reason)"
    pkgs="WORKSPACE"
elif [ -z "$python" ]; then
    echo "check: no Python 3 (tried python3, py -3, python) to scope the change — the whole workspace"
    pkgs="WORKSPACE"
else
    pkgs="$($python - "$root" $dirlist <<'PY'
import json, subprocess, sys, os
root, dirs = sys.argv[1], set(sys.argv[2:])
meta = json.loads(subprocess.run(
    ["cargo", "metadata", "--format-version", "1", "--no-deps"],
    capture_output=True, text=True, cwd=root, check=True).stdout)
by_dir, deps = {}, {}
names = {p["name"] for p in meta["packages"]}
for p in meta["packages"]:
    by_dir[os.path.relpath(os.path.dirname(p["manifest_path"]), root).replace(os.sep, "/")] = p["name"]
    deps[p["name"]] = {d["name"] for d in p["dependencies"] if d["name"] in names}
changed = {by_dir[d] for d in dirs if d in by_dir}
if any(d not in by_dir for d in dirs):  # a changed dir cargo doesn't know: not our map's to scope
    print("WORKSPACE"); sys.exit(0)
scope = set(changed)
grew = True
while grew:
    grew = False
    for name, ds in deps.items():
        if name not in scope and ds & scope:
            scope.add(name); grew = True
print(" ".join(sorted(scope)))
PY
)" || { echo "check: cargo metadata failed — scoping the whole workspace"; pkgs="WORKSPACE"; }
fi

in_scope() { [ "$pkgs" = "WORKSPACE" ] || [[ " $pkgs " == *" $1 "* ]]; }
if [ "$pkgs" = "WORKSPACE" ]; then
    pflags="--workspace"
else
    pflags=""
    for p in $pkgs; do pflags="$pflags -p $p"; done
fi
echo "check: scope = $pkgs"

log="$(mktemp "${TMPDIR:-/tmp}/benilla-check.XXXXXX")"
skips="$(mktemp "${TMPDIR:-/tmp}/benilla-check-skips.XXXXXX")"
trap 'rm -f "$log" "$skips"' EXIT
# Each gate logs to a file tailed afterwards: piping through `tail` would mask its exit code.
run() {
    local name="$1"; shift
    if ! "$@" >"$log" 2>&1; then
        echo "CHECK FAILED: $name"
        tail -30 "$log"
        exit 1
    fi
    echo "check ok: $name"
}
# libtest hides a passing test's output, so `install.rs`'s `skipped` also appends each data-gated
# skip to `$BENILLA_SKIP_LOG`; this prints them, grouped, after a test rung and empties the file.
report_skips() { # $1 = rung name
    [ -s "$skips" ] || return 0
    echo "  $1: $(wc -l <"$skips" | tr -d ' ') data-gated tests SKIPPED on this machine —"
    sort "$skips" | uniq -c | sort -rn | sed 's/^ *\([0-9]*\) \(.*\)/    \1 × \2/'
    echo "    (they run where the data is: docs/CONTRIBUTING.md, \"Setting up\")"
    : >"$skips"
}

run fmt cargo fmt --all -- --check
run clippy cargo clippy $pflags --all-targets -- -D warnings

# ── The data ─────────────────────────────────────────────────────────────────────────────────────
# The client's own resolver names the install, built beside `benilla` to reuse its dependencies;
# the corpus is the override, else the checkout's link.
wow_data=""
if cargo build -q -p benilla -p benilla-formats --example where >/dev/null 2>&1; then
    wow_data="$("${CARGO_TARGET_DIR:-target}/debug/examples/where" 2>/dev/null || true)"
fi
corpus="${BENILLA_ADDON_CORPUS-}"
[ -z "${BENILLA_ADDON_CORPUS+set}" ] && [ -d "$root/wow-addons-vanilla" ] && corpus="$root/wow-addons-vanilla"
[ -n "$corpus" ] && [ ! -d "$corpus" ] && corpus=""
require_data=""
if [ -n "$wow_data" ] && [ -n "$corpus" ]; then
    require_data=1
else
    echo "check: NOTE — install or addon corpus not found here (install='${wow_data:-none}'," \
         "corpus='${corpus:-none}'): data-gated tests skip on this machine"
fi

run test env ${require_data:+BENILLA_REQUIRE_DATA=1} BENILLA_SKIP_LOG="$skips" cargo test $pflags
report_skips test

# player-tests: `benilla-app` without `dev` with the data, for the shipped-UI tests that read
# FrameXML off the install. Without `dev` only $BENILLA_ADDON_CORPUS finds the corpus.
if in_scope benilla-app || in_scope benilla-formats; then
    run player-tests env ${wow_data:+WOW_DATA="$wow_data"} ${corpus:+BENILLA_ADDON_CORPUS="$corpus"} \
        ${require_data:+BENILLA_REQUIRE_DATA=1} BENILLA_SKIP_LOG="$skips" \
        cargo test -p benilla-formats -p benilla-app --no-default-features --lib
    report_skips player-tests
fi

# enforcer: `benilla-worldview` boots the engine plugins against the install with no server,
# login, UI or player, in a small window, and refuses any system whose parameters need a gameplay
# resource. The land gates boot it with no data only.
if in_scope benilla-worldview; then
    if [ -n "$wow_data" ]; then
        # Beside `benilla`, so it reuses the play build's dependencies (gates.sh says why).
        run enforcer-build cargo build -p benilla -p benilla-worldview
        run enforcer env WOW_DATA="$wow_data" WOW_WORLDVIEW_CHECK=10 \
            "${CARGO_TARGET_DIR:-target}/debug/benilla-worldview"
    else
        echo "check SKIPPED: enforcer (no WoW install found;" \
             "\`cargo run -p benilla-formats --example where\` says where it looked)"
    fi
fi

echo "CHECK GREEN (scope: $pkgs)"
echo "  (the land gates, scripts/gates.sh, run the whole workspace without the data before a pull request)"
