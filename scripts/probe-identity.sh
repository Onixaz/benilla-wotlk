#!/usr/bin/env bash
# Who a scripted run logs in as; sourced by smoke.sh, cine.sh and summon-live.sh, never run alone.
#
# A login kicks whoever holds the account, so there is no default: the checkout's `.probe-identity`
# (tree root, uncommitted; `WOW_USER=…`, `WOW_PASS=…`, `WOW_CHAR=…`, one per line), else the
# shell's WOW_USER, WOW_PASS and WOW_CHAR. The client refuses a scripted login on any account but
# the declared one (`run_mode::account_guard`).
#
# `probe_identity <tag> <tree root>` sets PROBE_USER, PROBE_PASS, PROBE_CHAR, PROBE_DECLARED
# (non-empty when the file supplied them) and PROBE_HOST, PROBE_AUTH_PORT from WOW_HOST (default
# localhost:3724), and prints the identity it chose; a refusal prints why and returns 1.
probe_identity() {
    local tag="$1" root="$2" file="$2/.probe-identity" v ignored=""
    PROBE_DECLARED=""
    if [ -f "$file" ]; then
        PROBE_USER="$(sed -n 's/^WOW_USER=//p' "$file" | head -1)"
        PROBE_PASS="$(sed -n 's/^WOW_PASS=//p' "$file" | head -1)"
        PROBE_CHAR="$(sed -n 's/^WOW_CHAR=//p' "$file" | head -1)"
        if [ -z "$PROBE_USER" ] || [ -z "$PROBE_PASS" ] || [ -z "$PROBE_CHAR" ]; then
            echo "$tag: REFUSING — $file is missing one of WOW_USER, WOW_PASS, WOW_CHAR"
            return 1
        fi
        PROBE_DECLARED=1
        for v in WOW_USER WOW_PASS WOW_CHAR; do
            [ -n "${!v:-}" ] && ignored="$ignored $v"
        done
        [ -n "$ignored" ] && echo "$tag: ignoring$ignored — this checkout declares its own account (.probe-identity)"
        echo "$tag: $PROBE_USER / $PROBE_CHAR (declared by .probe-identity)"
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
