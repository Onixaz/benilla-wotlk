#!/usr/bin/env bash
# Decode the install's movies with benilla-cinematics and with ffmpeg, and diff them frame by
# frame: every output frame's Adler-32 over the cropped Y, U and V planes, in output order.
#
#   scripts/movie-oracle.sh                       # every .avi under Data/<locale>/Interface/Cinematics
#   scripts/movie-oracle.sh path/to/WOW_Intro_LK_800.avi
#
# ffmpeg is the oracle, with its C integer IDCT (`-idct simple`) and `-flags +bitexact`. A file
# prints `ok N frames` when every frame matches, else the first differing frame. ffmpeg is a
# development tool here, never a dependency.
set -euo pipefail
cd "$(dirname "$0")/.."

ffmpeg=${FFMPEG:-$(command -v ffmpeg || echo /opt/homebrew/bin/ffmpeg)}
[ -x "$ffmpeg" ] || { echo "movie-oracle: no ffmpeg (set FFMPEG)" >&2; exit 2; }

if [ $# -gt 0 ]; then
    files=("$@")
else
    data=${WOW_DATA:-../wotlk-original/Data}
    files=()
    while IFS= read -r f; do files+=("$f"); done < <(find "$data" -ipath '*/Interface/Cinematics/*.avi' | sort)
fi
[ ${#files[@]} -gt 0 ] || { echo "movie-oracle: no movies found" >&2; exit 2; }

cargo build -q -p benilla-cinematics --example frame_hashes
bin=${CARGO_TARGET_DIR:-target}/debug/examples/frame_hashes
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail=0
for f in "${files[@]}"; do
    name=$(basename "$f")
    (
        "$ffmpeg" -v error -flags +bitexact -idct simple -i "$f" -map 0:v -fps_mode passthrough \
            -f framehash -hash adler32 - | grep -v '^#' | awk -F', *' '{print $6}' >"$tmp/$name.ref"
    ) &
    ( "$bin" "$f" >"$tmp/$name.ours" 2>"$tmp/$name.err" || true ) &
    wait
    ref=$(wc -l <"$tmp/$name.ref" | tr -d ' ')
    ours=$(wc -l <"$tmp/$name.ours" | tr -d ' ')
    first=$(paste "$tmp/$name.ref" "$tmp/$name.ours" | awk '$1 != $2 { print NR - 1; exit }')
    if [ -z "$first" ] && [ "$ref" = "$ours" ] && [ ! -s "$tmp/$name.err" ]; then
        echo "$name: ok $ref frames"
    else
        fail=1
        echo "$name: DIFFERS: ffmpeg $ref frames, ours $ours, first differing frame ${first:-none} $(cat "$tmp/$name.err")"
    fi
done
exit $fail
