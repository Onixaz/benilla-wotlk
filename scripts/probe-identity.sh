#!/usr/bin/env bash
# Who a scripted run logs in as; sourced by smoke.sh, cine.sh and summon-live.sh, never run alone.
#
# A login kicks whoever holds the account, so there is no default: the checkout's `.probe-identity`
# (tree root, uncommitted; `WOW_USER=…`, `WOW_PASS=…`, `WOW_CHAR=…`, one per line), else the
# shell's WOW_USER, WOW_PASS and WOW_CHAR. The lines before any `[name]` header are the default
# entry, else the first section is; WOW_PROBE_AS=<name> takes a `[name]` section instead. The client refuses a scripted login
# on an account the file does not declare, or one another run on this machine holds
# (`run_mode::fast_path`); `WOW_PROBE_AS=pool`, a free entry, is the client's alone, since only
# the client holds the lease.
#
# `probe_identity <tag> <tree root>` sets PROBE_USER, PROBE_PASS, PROBE_CHAR, PROBE_DECLARED
# (non-empty when the file supplied them) and PROBE_HOST, PROBE_AUTH_PORT from WOW_HOST (default
# localhost:3724), and prints the identity it chose; a refusal prints why and returns 1.
probe_identity() {
    local tag="$1" root="$2" file="$2/.probe-identity" v ignored="" entry="${WOW_PROBE_AS:-default}"
    PROBE_DECLARED=""
    if [ -f "$file" ]; then
        if [ "$entry" = pool ]; then
            echo "$tag: REFUSING — WOW_PROBE_AS=pool leases a free entry inside the client, which a"
            echo "       script that passes the account by name cannot do; name the entry instead."
            return 1
        fi
        # A file that is all sections defaults to its first.
        if [ -z "${WOW_PROBE_AS:-}" ] && [ -z "$(probe_identity_field "$file" default WOW_USER)" ]; then
            entry="$(sed -n 's/^[[:space:]]*\[[[:space:]]*\([^]]*[^][:space:]]\)[[:space:]]*\][[:space:]]*\r\{0,1\}$/\1/p' "$file" | head -1)"
            entry="${entry:-default}"
        fi
        PROBE_USER="$(probe_identity_field "$file" "$entry" WOW_USER)"
        PROBE_PASS="$(probe_identity_field "$file" "$entry" WOW_PASS)"
        PROBE_CHAR="$(probe_identity_field "$file" "$entry" WOW_CHAR)"
        if [ -z "$PROBE_USER" ] || [ -z "$PROBE_PASS" ] || [ -z "$PROBE_CHAR" ]; then
            echo "$tag: REFUSING — entry \`$entry\` of $file is missing one of WOW_USER, WOW_PASS, WOW_CHAR"
            return 1
        fi
        PROBE_DECLARED=1
        for v in WOW_USER WOW_PASS WOW_CHAR; do
            [ -n "${!v:-}" ] && ignored="$ignored $v"
        done
        [ -n "$ignored" ] && echo "$tag: ignoring$ignored — this checkout declares its own account (.probe-identity)"
        echo "$tag: $PROBE_USER / $PROBE_CHAR (entry \`$entry\` of .probe-identity)"
    else
        if [ -z "${WOW_USER:-}" ] || [ -z "${WOW_PASS:-}" ] || [ -z "${WOW_CHAR:-}" ]; then
            echo "$tag: REFUSING — no account to log in as. A scripted run takes WOW_USER, WOW_PASS"
            echo "       and WOW_CHAR from the environment, all three, or from a .probe-identity file"
            echo "       at the tree root, naming a test account on your server whose login kicks"
            echo "       nobody (docs/CONTRIBUTING.md, \"Setting up\")."
            return 1
        fi
        PROBE_USER="$WOW_USER"
        PROBE_PASS="$WOW_PASS"
        PROBE_CHAR="$WOW_CHAR"
        echo "$tag: $PROBE_USER / $PROBE_CHAR (from the environment)"
    fi
    # The server: WOW_HOST is `host` or `host:port`, the client's own spelling (realmlist.rs).
    local host="${WOW_HOST:-localhost}"
    PROBE_HOST="${host%%:*}"
    PROBE_AUTH_PORT="${host##*:}"
    [ "$PROBE_AUTH_PORT" != "$host" ] || PROBE_AUTH_PORT=3724
    return 0
}

# `probe_identity_field <file> <entry> <key>` prints the entry's first non-empty value for the key;
# `default` is the lines before any `[name]` header, names match case-blind, CRLF reads as LF.
probe_identity_field() {
    awk -v want="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]')" -v key="$3" '
        { sub(/\r$/, ""); sub(/^[ \t]+/, ""); sub(/[ \t]+$/, "") }
        BEGIN { cur = "default" }
        /^\[.*\]$/ { cur = tolower(substr($0, 2, length($0) - 2)); gsub(/^[ \t]+|[ \t]+$/, "", cur); next }
        cur == want && index($0, key "=") == 1 {
            v = substr($0, length(key) + 2); gsub(/^[ \t"]+|[ \t"]+$/, "", v)
            if (v != "") { print v; exit }
        }' "$1"
}

# `probe_server_or_skip <tag>` prints a skip and returns 1 when the auth port does not answer, or,
# for a local server, the stock world port 8085; a remote world port comes from its realm list.
probe_server_or_skip() {
    local tag="$1" port ports="$PROBE_AUTH_PORT"
    case "$PROBE_HOST" in localhost | 127.0.0.1) ports="$ports 8085" ;; esac
    for port in $ports; do
        if ! (exec 3<>"/dev/tcp/$PROBE_HOST/$port") 2>/dev/null; then
            echo "$tag: SKIPPED — nothing listening on $PROBE_HOST:$port (auth $PROBE_AUTH_PORT; a local"
            echo "       server also serves the world on 8085). Start your 3.3.5a server, or point"
            echo "       WOW_HOST at it."
            return 1
        fi
    done
    return 0
}
