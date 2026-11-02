#!/bin/zsh
# Client of the debug bridge (Sources/Ratchet/DebugBridge.swift, debug builds only). Writes one command into the running
# app's inbox and prints its JSON answer. Exit code 0 when the app answered ok, 1 when it said no, 2 when it did not answer.
#
#   RATCHET_HOME=<dir> tools/ratchetctl.sh status | open:<page|id> | appearance:light|dark | snapshot:<scene> | selftest
#
# <dir> is the app's $RATCHET_HOME (default: ~/Library/Application Support/Ratchet), the same one the app was started with.
# RATCHETCTL_TIMEOUT is the seconds to wait for the answer (default 20; the self-test needs about 60).
set -u
(( $# == 1 )) || { echo "usage: ratchetctl.sh <command>" >&2; exit 2; }
home="${RATCHET_HOME:-$HOME/Library/Application Support/Ratchet}"
bridge="$home/bridge"
[[ -d "$bridge/inbox" ]] || { echo "ratchetctl: no bridge at $bridge (is a debug build running? ./build.sh --debug)" >&2; exit 2; }
id="$$-$RANDOM$RANDOM$RANDOM"
# Write under a dot name and rename, so the app never reads half a command.
print -r -- "$1" > "$bridge/inbox/.$id.tmp"
mv "$bridge/inbox/.$id.tmp" "$bridge/inbox/$id.cmd"
reply="$bridge/outbox/$id.json"
deadline=$(( SECONDS + ${RATCHETCTL_TIMEOUT:-20} ))
while [[ ! -f "$reply" ]]; do
  if (( SECONDS >= deadline )); then
    rm -f "$bridge/inbox/$id.cmd"
    echo "ratchetctl: no answer to '$1' within ${RATCHETCTL_TIMEOUT:-20} s" >&2
    exit 2
  fi
  sleep 0.1
done
cat "$reply"
ok=$(plutil -extract ok raw -o - "$reply" 2>/dev/null)
rm -f "$reply"
[[ "$ok" == "true" ]]
