#!/usr/bin/env bash
# Play one cinematic on the probe and print what happened, as a timeline.
#
#   scripts/cine.sh                 # the human intro (81), parked in Stormwind
#   scripts/cine.sh 41              # the dwarf intro
#   scripts/cine.sh 81 --key W      # …and hold W mid-shot: does the body move?
#   scripts/cine.sh 81 --at "-8913,554,94,0"
#   scripts/cine.sh 81 --keep       # keep the raw log and trace paths, printed at the end
#
# Logs in as `.probe-identity`, else WOW_USER/WOW_PASS/WOW_CHAR. The body parks on flat ground
# first, and did-it-fall/did-it-move print beside the timeline: a wedged or falling body sends no
# movement. Sound is live but muted through a BENILLA_HOME config (`MasterVolume = "0"`), because
# under WOW_NOSOUND `zone::start_music_stream` returns before it logs.
set -euo pipefail
cd "$(dirname "$0")/.."

id=81
park="-8913,554,94,0"   # Stormwind Trade District: flat, solid, and a zone with music
key=""
keep=0
while [ $# -gt 0 ]; do
    case "$1" in
        --at)   park="$2"; shift 2 ;;
        --key)  key="$2"; shift 2 ;;
        --keep) keep=1; shift ;;
        -*)     echo "cine.sh: unknown flag $1" >&2; exit 2 ;;
        *)      id="$1"; shift ;;
    esac
done

. "$PWD/scripts/probe-identity.sh"
probe_identity cine.sh "$PWD" || exit 2
char="$PROBE_CHAR"

work="$(mktemp -d)"
cfg="$work/cfg"
mkdir -p "$cfg/WTF"
printf 'SET Sound_MasterVolume "0"\n' > "$cfg/WTF/Config.wtf"
log="$work/run.log"
trace="$work/move.trace"

IFS=, read -r px py pz pmap <<< "$park"

# The schedule, in probe-clock seconds; generous, because a cold start streams a city slowly.
park_at=8
play_at=20
key_at=$((play_at + 8))
# Shots run 25-102 s; leave room for the end, the resume and the cover to settle.
exit_at=$((play_at + 120))

echo "cine.sh: cinematic $id as $char, parked at $px $py $pz (map $pmap)"
[ -n "$key" ] && echo "cine.sh: holding $key for 3 s at t=${key_at}s"
echo "cine.sh: this takes about $((exit_at + 20)) s — the shot is played in full so the END is measured too"

env BENILLA_HOME="$cfg" WOW_UNATTENDED=1 \
    WOW_USER="$PROBE_USER" WOW_PASS="$PROBE_PASS" WOW_CHAR="$char" \
    WOW_PROBE_CHAT=".go xyz $px $py $pz $pmap;.debug play cinematic $id" \
    WOW_PROBE_CHAT_AT="$park_at" WOW_PROBE_CHAT_EVERY="$((play_at - park_at))" \
    ${key:+WOW_PROBE_KEY="$key@$key_at:3"} \
    WOW_MOVE_TRACE="$trace" WOW_MOVE_TRACE_TAGS="move,snd" \
    WOW_PROBE_EXIT_AT="$exit_at" \
    cargo run -q -p benilla > "$log" 2>&1 || true

strip() { sed -E 's/\x1b\[[0-9;]*m//g'; }

echo
echo "── timeline ────────────────────────────────────────────────────────────────"
strip < "$log" | grep -E \
    "cinematic:|screen fade:|cinematic voice:|zone music:|ambience:|loading screen:|body held|probe-key:" \
    | sed -E 's/^[0-9-]+T([0-9:.]+)Z +INFO +[a-z_:]+: /\1  /' || true

echo
echo "── the two control facts ───────────────────────────────────────────────────"
if [ -s "$trace" ]; then
    # Everything before the park is the login/teleport arc and says nothing about the cinematic.
    after=$(awk -v t="$park_at" '$1=="t=" || $2+0 >= t' "$trace" 2>/dev/null || cat "$trace")
    low=$(printf '%s\n' "$after" | grep -oE 'pos=\[[-0-9.]+,[-0-9.]+,[-0-9.]+\]' \
          | sed -E 's/.*,//; s/\]//' | sort -n | head -1)
    echo "did it FALL?  lowest z after the park: ${low:-<none>}   (park z was $pz)"
    verbs=$(printf '%s\n' "$after" | grep -oE 'snd  [A-Za-z_]+' | sort | uniq -c | tr '\n' ' ')
    echo "did it MOVE?  wire verbs sent: ${verbs:-<none>}"
    echo "              (a StartForward means the body walked; only Heartbeats means it did not)"
else
    echo "no move trace written — the run did not reach the world"
fi

echo
if [ "$keep" = 1 ]; then
    echo "log:   $log"
    echo "trace: $trace"
else
    rm -rf "$work"
fi
