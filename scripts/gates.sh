#!/usr/bin/env bash
# The land gate chain: every gate in order, stopping at the first failure.
#
# fmt, clippy, test, doc-links, pass-span-lint, player-tests, enforcer-build, enforcer-no-install.
# The chain is synthetic: it reads no WoW install and no addon corpus (`WOW_DATA=` and
# `BENILLA_ADDON_CORPUS=`, set and empty), so it runs the same on a fresh clone, a CI runner and a
# machine with the data.
# The tests that read the data are `scripts/check.sh`'s, per round. Everything builds in `dev`, the
# play build, so the chain reuses the artifacts a `cargo play` or a probe build left.
# A failure prints the gate's log tail and exits nonzero; a green run stamps the tree, which then
# re-greens without running. GATES_FORCE=1 runs the chain anyway; TMPDIR holds the scratch files.
# `.github/workflows/ci.yml` runs this script on a fork's pull request.
#
#   scripts/gates.sh
set -uo pipefail

# Gate the checkout $PWD is in, not this file's: a caller in one worktree may run another's copy
# by absolute path. Outside a checkout of this repo, gate this file's own checkout.
here="$(cd "$(dirname "$0")/.." && pwd)"
root="$(git rev-parse --show-toplevel 2>/dev/null || true)"
[ -n "$root" ] && [ -f "$root/scripts/gates.sh" ] || root="$here"
cd "$root" || exit 1
echo "gating: $root"

# No install and no corpus, whatever the caller's environment or the checkout's links say.
export WOW_DATA= BENILLA_ADDON_CORPUS=
unset BENILLA_REQUIRE_DATA BENILLA_SKIP_LOG

# The first Python 3 that runs: `python3` is a Microsoft Store stub on some Windows machines.
python=""
for p in python3 "py -3" python; do
    if $p -c 'import sys; sys.exit(sys.version_info < (3, 8))' >/dev/null 2>&1; then
        python="$p"
        break
    fi
done

# ── Green-stamp memoization ──────────────────────────────────────────────────────────────────────
# A green run stamps target/.gates-green with the working tree's hash (untracked files included,
# through a throwaway index that leaves the real one alone), rustc's version and this script's hash.
stamp="target/.gates-green"
tree_key() {
    local tmpidx t
    tmpidx="$(mktemp "${TMPDIR:-/tmp}/benilla-gates-idx.XXXXXX")" && rm -f "$tmpidx" || return 1
    t="$( (export GIT_INDEX_FILE="$tmpidx"
           git read-tree HEAD && git add -A . && git write-tree) 2>/dev/null )"
    rm -f "$tmpidx"
    [ -n "$t" ] || return 1
    printf '%s|%s|%s' "$t" "$(rustc -V 2>/dev/null)" \
        "$(shasum "$root/scripts/gates.sh" 2>/dev/null | cut -d' ' -f1)"
}
# A delta only under docs/ or in top-level *.md files keeps the stamped verdict, since no gate
# reads those paths; a gate that starts to read one must narrow this pattern.
docs_only_delta() { # $1 = stamped tree, $2 = current tree
    git cat-file -e "$1^{tree}" 2>/dev/null && git cat-file -e "$2^{tree}" 2>/dev/null || return 1
    [ -z "$(git diff-tree -r --name-only "$1" "$2" 2>/dev/null | grep -vE '^(docs/|[^/]*\.md$)' || true)" ]
}
key_start="$(tree_key || true)"
if [ "${GATES_FORCE:-}" != "1" ] && [ -n "$key_start" ] && [ -f "$stamp" ]; then
    old_key="$(cat "$stamp" 2>/dev/null)" # tree|rustc|script — all must match, bar a docs-only tree
    memo=""
    if [ "$old_key" = "$key_start" ]; then
        memo="this exact tree already passed"
    elif [ "${old_key#*|}" = "${key_start#*|}" ] && docs_only_delta "${old_key%%|*}" "${key_start%%|*}"; then
        memo="this tree differs from one that passed only under docs/ or in top-level *.md, which no gate reads"
    fi
    if [ -n "$memo" ]; then
        echo "ALL GATES GREEN (memoized — $memo; GATES_FORCE=1 re-runs)"
        [ -d target ] && printf '%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" \
            "$(git rev-parse --short HEAD 2>/dev/null || echo '-')" chain 0 memo >>target/.gates-timing 2>/dev/null
        echo "  (the data-gated tests are scripts/check.sh's; a clean run is scripts/smoke.sh)"
        exit 0
    fi
fi

log="$(mktemp "${TMPDIR:-/tmp}/benilla-gates.XXXXXX")"
trap 'rm -f "$log"' EXIT

# ── Timing ───────────────────────────────────────────────────────────────────────────────────────
# target/.gates-timing gets one tab-separated line per gate: UTC time, HEAD's short sha, gate,
# seconds, verdict. A memo hit is logged at 0 s, the whole chain as `chain`.
timing="target/.gates-timing"
chain_t0=$SECONDS
tree_sha="$(git rev-parse --short HEAD 2>/dev/null || echo '-')"
note_timing() { # $1 = gate, $2 = seconds, $3 = verdict
    [ -d target ] || return 0
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "$tree_sha" "$1" "$2" "$3" >>"$timing" 2>/dev/null || true
}

run() {
    local name="$1"
    shift
    local t0=$SECONDS
    if ! "$@" >"$log" 2>&1; then
        echo "GATE FAILED: $name ($((SECONDS - t0)) s)"
        note_timing "$name" "$((SECONDS - t0))" "FAILED"
        tail -30 "$log"
        exit 1
    fi
    echo "gate ok: $name ($((SECONDS - t0)) s)"
    note_timing "$name" "$((SECONDS - t0))" "ok"
}

run fmt cargo fmt --all -- --check
run clippy cargo clippy --workspace --all-targets -- -D warnings

# test: the workspace with no install, which also refuses a test that reads the install without
# `wow_data_or_skip!`: `WOW_DATA=` is the resolver's "no install" even where a `WoW` link is.
run test cargo test --workspace

if [ -z "$python" ]; then
    echo "GATE FAILED: doc-links (no Python 3: tried python3, py -3, python)"
    note_timing doc-links 0 FAILED
    exit 1
fi

# doc-links: refuses a doc link to a `crate::`, `super::`, `self::` or `Self::` path whose leaf is
# declared nowhere in the workspace. Narrow on purpose: every other unresolved link is a convention.
run doc-links $python scripts/doc-links.py

# pass-span-lint: refuses a second `pass_span` on a render pass whose span is still open, a fatal
# wgpu validation error on Vulkan and a silent no-op on Metal and DX12, which no macOS run sees.
run pass-span-lint $python scripts/pass-span-lint.py

# player-tests: the player build, `benilla-app` without `dev`, which compiles out the debug panel,
# perf HUD, inspector, capture harness and probes, so it fails when any other code names one of
# them. Its test build compiles every line of the player graph but the `benilla` launcher, a shim
# with no `dev` seam, so a `cargo build -p benilla --no-default-features` would add only the shim
# and a link. It runs the unit tests of the `cfg(not(feature = "dev"))` code (the resolver skips
# the source tree; the state folder sits beside the binary), which no other gate runs.
run player-tests cargo test -p benilla-formats -p benilla-app --no-default-features --lib

# enforcer-no-install: `benilla-worldview` boots the engine plugins with no server, login, UI,
# player or client data, in a small window, and refuses any system whose parameters need a resource
# inserted only by gameplay or by client data, a coupling no compiler sees. Five seconds suffice:
# with nothing to stream, every system validates in the first frames. CI runs it under Xvfb.
# Built beside `benilla`: alone, `benilla-worldview` resolves fewer dependency features and would
# rebuild Bevy rather than reuse the play build's.
run enforcer-build cargo build -p benilla -p benilla-worldview
run enforcer-no-install env WOW_WORLDVIEW_CHECK=5 "${CARGO_TARGET_DIR:-target}/debug/benilla-worldview"

echo "ALL GATES GREEN ($((SECONDS - chain_t0)) s; per-gate seconds in $timing)"
note_timing chain "$((SECONDS - chain_t0))" ok

# Stamp only if the tree still hashes as it did at the start: an edit during the run would
# otherwise memoize a green for a tree the chain never saw.
key_end="$(tree_key || true)"
if [ -n "$key_end" ] && [ "$key_end" = "$key_start" ] && [ -d target ]; then
    printf '%s\n' "$key_end" > "$stamp" 2>/dev/null || true
fi

# The data-gated tests and the engine boot against the install run per round; the clean-run gate
# needs a server and a window. Named here, not run.
echo "  (the data-gated tests are scripts/check.sh's; a clean run is scripts/smoke.sh)"
