#!/usr/bin/env bash
# Start the client in sound measuring mode: record the mix before and after the output limiter.
#
#   scripts/soundprobe.sh [client args…]   # play normally, press F9 when you hear it
#   scripts/soundprobe.py                  # …then read the capture back
#
# Builds `dev`, the optimized play build: an unoptimized build stutters, and a stutter is a missed
# mix deadline, one of the mechanisms the capture has to tell apart.
set -euo pipefail
cd "$(dirname "$0")/.."

cat <<'BANNER'

  ── benilla · sound measuring mode ─────────────────────────────────────────
    Recording two mixes at once: what the game ASKED for (before the limiter)
    and what you actually HEARD (after it). One file cannot tell those apart;
    two can, and that is the whole question.

    ▶ PRESS F9 THE MOMENT YOU HEAR IT.
      That stamps the exact sample. Without a mark I am scanning ten minutes
      of audio guessing which second you meant; with one I read outward from
      it. Press it every time — several marks is much better than one.

    Then quit the client normally and run:  scripts/soundprobe.py
  ───────────────────────────────────────────────────────────────────────────

BANNER

exec env WOW_SOUND_PROBE=1 cargo run -q -p benilla "$@"
