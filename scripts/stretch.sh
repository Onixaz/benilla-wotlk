#!/usr/bin/env bash
# A scripted quest stretch: the dev client logs in, runs scripts/stretches/<name>.lua through the
# stretch library, and exits with its verdict; the wire audit judges every packet on the way.
#
#   scripts/stretch.sh <name>|all [--watch]
#
# The character must be a Death Knight on the checkout's probe account: WOW_PROBE_AS names the
# .probe-identity entry, STRETCH_CHAR the character on it (default the entry's). WOW_HOST and
# WOW_DATA as for any run. --watch slows each step by STRETCH_BEAT seconds (default 1.5) so the run
# can be followed; keep the mouse off the window, as a real pointer overrides the probe's.
#
# Each run boots the stock interface on a fresh config root (no third-party addons, default
# bindings) and writes target/stretch/<name>-<stamp>/: client.log, trace.txt, the filmstrip (one shot
# per step, and every STRETCH_EVERY seconds, default 4) and index.html, which lays the shots along
# the steps and the wire audit's lines. Exit: 0 pass, 3 a wire rule broken, 4 no verdict by the
# timeout (STRETCH_TIMEOUT, default 600 s), 5 a failed step, 1 a setup problem.
set -uo pipefail

root="$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$root" ] || [ ! -f "$root/scripts/stretch.sh" ]; then
    root="$(cd "$(dirname "$0")/.." && pwd)"
fi
cd "$root" || exit 1

name="${1:-}"
watch=""
[ "${2:-}" = "--watch" ] && watch=1
if [ -z "$name" ]; then
    echo "usage: scripts/stretch.sh <name>|all [--watch]; stretches:"
    ls scripts/stretches | sed -n 's/\.lua$//p' | grep -v '^lib$' | sed 's/^/  /'
    exit 1
fi

if [ "$name" = all ]; then
    status=0
    for f in scripts/stretches/*.lua; do
        n="$(basename "$f" .lua)"
        [ "$n" = lib ] && continue
        "$0" "$n" ${watch:+--watch}
        rc=$?
        echo "stretch $n: exit $rc"
        [ $rc -ne 0 ] && status=$rc
    done
    exit $status
fi

script="scripts/stretches/$name.lua"
if [ ! -f "$script" ]; then
    echo "stretch: no $script"
    exit 1
fi

. "$root/scripts/probe-identity.sh"
probe_identity stretch "$root" || exit 1

if ! cargo build -q -p benilla; then
    echo "stretch: the client did not build"
    exit 1
fi

out="$root/target/stretch/$name-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$out"
# A fresh local-state root per run: the stock interface alone, default key bindings and no saved
# state from an earlier run; only the realm the checkout's own config names is carried over.
home="$out/home"
mkdir -p "$home/WTF"
shared="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)/../benilla-config/WTF/Config.wtf"
[ -f "$shared" ] && grep -i '^SET realmName' "$shared" >"$home/WTF/Config.wtf"
beat=""
[ -n "$watch" ] && beat="STRETCH_BEAT = ${STRETCH_BEAT:-1.5}"
chunk="$beat
$(cat scripts/stretches/lib.lua)
$(cat "$script")"

echo "stretch: $name -> $out"
env BENILLA_HOME="$home" WOW_STRETCH="$name" WOW_STRETCH_DIR="$out" WOW_STRETCH_EVERY="${STRETCH_EVERY:-4}" \
    WOW_UNATTENDED=1 WOW_NOSOUND=1 WOW_MOVE_TRACE="$out/trace.txt" \
    WOW_PROBE_LUA="$chunk" WOW_PROBE_LUA_AT=5 WOW_PROBE_EXIT_AT="${STRETCH_TIMEOUT:-600}" \
    ${STRETCH_CHAR:+WOW_CHAR="$STRETCH_CHAR"} \
    ./target/debug/benilla >"$out/client.log" 2>&1
rc=$?

clean() { sed -E 's/\x1b\[[0-9;]*m//g' "$out/client.log"; }
clean | grep -E "stretch-step:|stretch-verdict:|stretch: FAIL|wire-audit:|preflight:" |
    sed -E 's/^[^ ]+ +[A-Z]+ +[^ ]+: //'
python3 "$root/scripts/stretch_report.py" "$out" >/dev/null 2>&1 ||
    py -I "$root/scripts/stretch_report.py" "$out" >/dev/null 2>&1 || true
case $rc in
0) echo "STRETCH $name PASSED" ;;
3) echo "STRETCH $name FAILED: a wire rule broken (see wire-audit lines)" ;;
4) echo "STRETCH $name FAILED: no verdict before the timeout" ;;
5) echo "STRETCH $name FAILED: a step failed" ;;
*) echo "STRETCH $name FAILED: client exit $rc" ;;
esac
[ -f "$out/index.html" ] && echo "filmstrip: $out/index.html"
exit $rc
