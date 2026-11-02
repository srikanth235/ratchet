#!/usr/bin/env bash
# The CLI contract on a built binary (the one inside the app bundle in CI): --help lists every command, each command has help,
# --json parses, a wrong id exits 1 with a hint, and a command that changes things needs --yes. The same checks run in-process
# in tests/cli/contract.rs; this is the check that the shipped binary behaves the same.
#
#   crates/ratchet-cli/tests/contract.sh <path to ratchet> <path to ratchetd>
set -u
ratchet="${1:?path to the ratchet binary}"
ratchetd="${2:?path to the ratchetd binary}"
fail=0
ok()   { echo "CONTRACT ok   $*"; }
bad()  { echo "CONTRACT FAIL $*"; fail=1; }

commands="validate init baseline templates status list show run stop report pr candidates history spend activity agent events chat env cost repos inspect overview connectors open mcp"

# 1. --help lists every command, and nothing the contract does not know.
help=$("$ratchet" --help) || bad "--help exits non-zero"
listed=$(printf '%s\n' "$help" | awk '/^Commands:/{b=1;next} b&&/^  /{gsub(",","",$1);print $1;next} b{exit}')
for c in $commands; do
  grep -qx "$c" <<<"$listed" && ok "--help lists $c" || bad "--help does not list $c"
done
for c in $listed; do
  [[ " $commands help " == *" $c "* ]] || bad "--help lists '$c', which the contract does not know"
done

# 2. every command has help of its own.
for c in $commands; do
  "$ratchet" "$c" --help >/dev/null 2>&1 && ok "$c --help" || bad "$c --help exits non-zero"
done

# 3. against a simulation daemon: --json parses, a wrong id is exit 1 with a hint, --yes is required.
home=$(mktemp -d /tmp/ratchet-contract.XXXXXX)
export RATCHET_HOME="$home" RATCHET_NO_AUTOSTART=1
"$ratchetd" serve --mode sim --ledger :memory: >"$home/daemon.log" 2>&1 &
daemon=$!
trap 'kill $daemon 2>/dev/null; wait $daemon 2>/dev/null; rm -rf "$home"' EXIT
for _ in $(seq 1 100); do [[ -S "$home/ratchetd.sock" && -s "$home/ratchetd.token" ]] && break; sleep 0.2; done
[[ -S "$home/ratchetd.sock" ]] || { bad "the daemon did not open its socket"; cat "$home/daemon.log"; exit 1; }

is_json() { python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; }
for args in "status" "list" "overview" "connectors" "env list" "activity" "templates"; do
  # shellcheck disable=SC2086
  "$ratchet" $args --json | is_json && ok "$args --json parses" || bad "$args --json is not JSON"
done

"$ratchet" show no-such-id >/dev/null 2>"$home/err.txt"; code=$?
(( code == 1 )) && ok "a wrong id exits 1" || bad "a wrong id exits $code, not 1"
grep -q "hint:" "$home/err.txt" && ok "a wrong id prints a hint" || bad "a wrong id prints no hint: $(cat "$home/err.txt")"
"$ratchet" show no-such-id --json >/dev/null 2>"$home/err.json"
is_json <"$home/err.json" && ok "the error is JSON with --json" || bad "the error is not JSON: $(cat "$home/err.json")"

"$ratchet" stop opt_demo_search >/dev/null 2>&1; code=$?
(( code == 2 )) && ok "stop without --yes exits 2" || bad "stop without --yes exits $code, not 2"

"$ratchet" open connectors --print | grep -qx "ratchet://open/connectors" && ok "open --print" || bad "open --print"

(( fail )) && { echo "CONTRACT FAILED"; exit 1; }
echo "CONTRACT PASS"
