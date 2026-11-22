#!/usr/bin/env bash
# Generate docs/MAP.md, the map of what is built, from what is on disk.
#
# Regenerated and committed at every land; run it by hand only to look, never edit the output. No
# timestamps, so an identical tree gives an identical map. No `pipefail`/`-e`: a grep that finds
# nothing (a single-file lib, a crate with no bins) must not abort the generator.
set -u
cd "$(dirname "$0")/.."
# One byte order for globs, `sort` and `awk` on every host, so the same tree gives the same map.
export LC_ALL=C

# Extract a quoted `key = "value"` field from a Cargo.toml.
field() { grep -m1 "^$2" "$1" 2>/dev/null | sed -E "s/^$2[[:space:]]*=[[:space:]]*\"//; s/\".*$//"; }

# The crates that build Bevy, one per line: a crate whose `[dependencies]` (target-specific ones
# included, dev- and build-dependencies not) name Bevy or avian3d, or a workspace crate that does.
bevy_crates() {
  awk '
    FNR == 1 { split(FILENAME, p, "/"); c = p[2]; deps = 0 }
    /^\[/ { deps = ($0 ~ /^\[(target\..*\.)?dependencies\]$/); next }
    deps && /^(bevy|bevy_[a-z_]+|avian3d)[.[:space:]=]/ { bevy[c] = 1 }
    deps && match($0, /^benilla[-a-z0-9]*[.[:space:]=]/) {
      d = substr($0, 1, RLENGTH - 1); edge[++n] = c SUBSEP d
    }
    END {
      do {
        grew = 0
        for (i = 1; i <= n; i++) {
          split(edge[i], e, SUBSEP)
          if ((e[2] in bevy) && !(e[1] in bevy)) { bevy[e[1]] = 1; grew = 1 }
        }
      } while (grew)
      for (c in bevy) print c
    }' crates/*/Cargo.toml
}

# The plugins a file registers, in order, one `- Name` line each: its `add_plugins(…)` calls, or
# with a group name the `.add(…)`/`.add_group(…)` members of that `impl PluginGroup`'s `build`. A
# type keeps its last path segment (`ui_movie::player::MoviePlayerPlugin` → `MoviePlayerPlugin`),
# a function plugin its module too (`crate::shaders::plugin` → `shaders::plugin`).
plugins() {
  awk -v grp="${2:-}" '
    BEGIN { pat = grp == "" ? "add_plugins\\(" : "\\.add(_group)?\\(" }
    grp != "" && !on { if (index($0, "impl PluginGroup for " grp " ")) on = 1; next }
    grp != "" && /^    }$/ { exit }
    /^[[:space:]]*\/\// { next }
    { line = held $0; held = "" }
    line ~ (pat "[[:space:]]*$") { held = line; next }
    match(line, pat "[[:space:]]*[A-Za-z_][A-Za-z0-9_:<>]*") {
      s = substr(line, RSTART, RLENGTH); sub(/^[^(]*\([[:space:]]*/, "", s)
      sub(/::(default|new)$/, "", s); gsub(/::</, "<", s)
      n = split(s, p, "::"); s = p[n]
      if (s ~ /^[a-z]/ && n > 1) s = p[n - 1] "::" s
      if (!seen[s]++) print "- " s
    }' "$1"
}

{
  echo "# benilla — generated map"
  echo
  echo "> **GENERATED** by \`scripts/genmap.sh\` from the code — do not edit by hand; rerun the script"
  echo "> when what's built changes. *What-is* only: the *what-changed* is in git. No timestamp —"
  echo "> identical code yields an identical map."
  echo

  echo "## Crates"
  echo
  echo "> Bevy: the crate builds Bevy, itself or through a workspace crate it depends on."
  echo
  bevy_set=" $(bevy_crates | tr '\n' ' ')"
  for ct in crates/*/Cargo.toml; do
    name=$(field "$ct" name)
    desc=$(field "$ct" description)
    dir=$(echo "$ct" | sed -E 's@crates/([^/]+)/.*@\1@')
    case "$bevy_set" in *" $dir "*) bevy="Bevy" ;; *) bevy="no Bevy" ;; esac
    echo "- **$name** ($bevy) — ${desc:-—}"
  done
  echo

  echo "## App subsystems — Bevy plugins in load order (\`crates/benilla-app/src/lib.rs\`)"
  echo
  plugins crates/benilla-app/src/lib.rs
  echo

  # `lib.rs` adds each group in one line, so their members are read out of the group's `build`.
  echo "### \`WorldPlugins\`, the engine (\`crates/benilla-world/src/world_plugins.rs\`)"
  echo
  plugins crates/benilla-world/src/world_plugins.rs WorldPlugins
  echo

  echo "### \`GamePlugins\`, the game (\`crates/benilla-app/src/game_plugins.rs\`)"
  echo
  plugins crates/benilla-app/src/game_plugins.rs GamePlugins
  echo

  echo "### \`DevToolsPlugin\` and \`DevProbesPlugin\`, behind the \`dev\` feature (\`crates/benilla-app/src/dev.rs\` — compiled out by \`--no-default-features\`)"
  echo
  plugins crates/benilla-app/src/dev.rs
  echo

  echo "## Modules (top-level, per crate)"
  echo
  echo "> \`(dev)\`: compiled only with the \`dev\` feature. Test-only modules are left out."
  echo
  for lib in crates/*/src/lib.rs crates/*/src/main.rs; do
    [ -f "$lib" ] || continue
    crate=$(echo "$lib" | sed -E 's@crates/([^/]+)/.*@\1@')
    # A `#[cfg(…)]` line binds the `mod` it heads; other attributes between them keep it bound.
    mods=$(awk '
      /^[[:space:]]*#\[cfg\(test\)\]/ { test = 1; next }
      /^[[:space:]]*#\[cfg\(feature = "dev"\)\]/ { dev = 1; next }
      /^[[:space:]]*(pub(\([a-z]+\))? )?mod [a-z_0-9]+;/ {
        name = $0; sub(/;.*/, "", name); sub(/.*mod /, "", name)
        if (!test) print name (dev ? " (dev)" : "")
        test = dev = 0; next
      }
      /^[[:space:]]*#\[/ { next }
      { test = dev = 0 }' "$lib" | sort | awk '{ printf "%s%s", (NR > 1 ? ", " : ""), $0 }')
    [ -n "$mods" ] && echo "- **$crate** ($(basename "$lib")): $mods"
  done
  echo

  echo "## Binaries"
  echo
  # Cargo's three forms: `src/main.rs` (named after the package; every `[[bin]]` here names its
  # package's `src/main.rs`), `src/bin/<name>.rs` and `src/bin/<name>/main.rs`.
  for b in crates/*/src/main.rs crates/*/src/bin/*.rs crates/*/src/bin/*/main.rs; do
    [ -f "$b" ] || continue
    crate=$(echo "$b" | sed -E 's@crates/([^/]+)/.*@\1@')
    case "$b" in
      */src/main.rs) name=$(field "crates/$crate/Cargo.toml" name) ;;
      */main.rs) name=$(basename "$(dirname "$b")") ;;
      *) name=$(basename "$b" .rs) ;;
    esac
    printf '%s\t%s\n' "$name" "$crate"
  done | sort | awk -F'\t' '{ print "- `" $1 "` (in " $2 ")" }'
  echo

  echo "## Examples (\`cargo run -p <crate> --example <name>\`)"
  echo
  for d in crates/*/examples; do
    [ -d "$d" ] || continue
    crate=$(echo "$d" | sed -E 's@crates/([^/]+)/.*@\1@')
    ex=$(for e in "$d"/*.rs "$d"/*/main.rs; do
      [ -f "$e" ] || continue
      case "$e" in */main.rs) basename "$(dirname "$e")" ;; *) basename "$e" .rs ;; esac
    done | sort | awk '{ printf "%s%s", (NR > 1 ? ", " : ""), $0 }')
    [ -n "$ex" ] && echo "- **$crate**: $ex"
  done
  echo

  # Shaders are embedded and addressed by their owning crate, so the crate is part of the name.
  echo "## WGSL shaders (\`crates/*/src/shaders/\`, embedded)"
  echo
  for s in crates/*/src/shaders/*.wgsl; do
    [ -f "$s" ] || continue
    crate=$(echo "$s" | sed -E 's@crates/([^/]+)/.*@\1@' | tr - _)
    echo "- \`embedded://$crate/shaders/$(basename "$s")\`"
  done
  echo

  echo "## Instruments — \`\$WOW_*\` switches (env-var read sites)"
  echo
  echo "> The headless/dev instrument fleet, discovered from the code: every \`WOW_*\` env var"
  echo "> some \`.rs\` reads, and where it's read — the doc comment at the read site is the"
  # Name no dev keys here: they are listed once, in the debug panel's footer.
  echo "> semantics. (The in-window surfaces — the Ctrl+Shift dev-chord overlays — are plugins"
  echo "> above; this indexes the switches that don't announce themselves.)"
  echo
  # Match any quoted "WOW_*" literal: reads also go through helpers (`knob("WOW_FX_AGE", …)`), and
  # doc comments write the unquoted form. A literal ending in `_` is a prefix match, not a switch.
  # The probe registry `capture/probe_env.rs` is a table, not a read site, so it is left out.
  grep -rHoE '"WOW_[A-Z0-9_]*[A-Z0-9]"' crates --include='*.rs' 2>/dev/null \
    | grep -v '^crates/benilla-app/src/capture/probe_env\.rs:' \
    | sed -E 's@^crates/@@; s/:"/\t/; s/"$//' \
    | awk -F'\t' '{print $2 "\t" $1}' | sort -u \
    | awk -F'\t' '$1 != v { if (v) print line; v = $1; line = "- `" $1 "` — " $2; next }
                  { line = line ", " $2 }
                  END { if (v) print line }'
  echo

  # Every script, with the first sentence of its own header.
  echo "## Scripts (\`scripts/\`)"
  echo
  for f in scripts/*.py scripts/*.sh scripts/*/*.py scripts/*/*.sh scripts/*/*.lua; do
    [ -f "$f" ] || continue
    # The summary: the header's first non-empty content after the shebang, comment or docstring
    # markers stripped, joined until its first blank line, cut at a sentence end or ~150 chars.
    summary=$(awk '
      NR == 1 && /^#!/ { next }
      /^@echo off/ { next }
      { line = $0
        sub(/^[[:space:]]*(r?"""|#|\/\/|--|rem)[[:space:]]?/, "", line)
        if (line ~ /^[[:space:]]*$/) { if (got) exit; else next }
        if (line ~ /^"""/) exit
        got = 1; out = out (out ? " " : "") line }
      END { print out }' "$f" | sed -E 's/\*\*//g; s/^[A-Za-z0-9_.\/-]+ (—|--) //; s/^([^.]*[.!?])([[:space:]]|$).*/\1/; s/^(.{150}).+/\1…/')
    echo "- \`${f#scripts/}\` — $summary"
  done
  echo

} > docs/MAP.md

echo "genmap: wrote docs/MAP.md"
