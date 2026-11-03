#!/bin/zsh
# Drives the debug bridge end to end (test-mac-app skill, level 3): starts a DEBUG build of the app in test mode (sample
# data, a private RATCHET_HOME), then sends it commands with tools/ratchetctl.sh and checks the answers. Exit code 0 when
# every check passed. Needs a debug build: RATCHET_SKIP_DAEMON=1 ./build.sh --debug.
#   tools/bridge-test.sh [path to Ratchet.app]
set -u
here=${0:A:h}
app="${1:-$HOME/Applications/Ratchet.app}"
bin="$app/Contents/MacOS/Ratchet"
fail=0
note() { echo "BRIDGE $*"; }
check() { if eval "$2"; then note "ok   $1"; else note "FAIL $1"; fail=1; fi }

[[ -x "$bin" ]] || { note "FAIL no app at $app"; exit 1; }
# A short path: nothing here needs a socket, but the daemon helpers in the same folder do.
home=$(mktemp -d /tmp/ratchet-bridge.XXXXXX)
store=/tmp/Ratchet-bridge.store
rm -f $store $store-shm $store-wal
rm -rf ~/Library/Saved\ Application\ State/dev.ratchet.app.savedState
RATCHET_HOME="$home" APP_STORE=$store "$bin" >"$home/app.log" 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null; wait $pid 2>/dev/null; rm -rf "$home"' EXIT

for _ in {1..150}; do [[ -f "$home/bridge/ready" ]] && break; kill -0 $pid 2>/dev/null || break; sleep 0.2; done
if [[ ! -f "$home/bridge/ready" ]]; then
  note "FAIL the app did not open the bridge (is this a debug build?)"
  tail -n 40 "$home/app.log"
  exit 1
fi
note "ok   the bridge is listening at $home/bridge"

ctl() { RATCHET_HOME="$home" "$here/ratchetctl.sh" "$@"; }
# field <json file> <key path>: plutil reads JSON.
field() { plutil -extract "$2" raw -o - "$1" 2>/dev/null; }
out="$home/reply.json"

# status: the app's own view of itself. The bridge listens as soon as the window exists; the store loads a moment later (it
# starts when the daemon phase is running), so wait for the sample data before reading the status that is checked.
for _ in {1..50}; do
  ctl status >"$out" 2>/dev/null
  (( $(field $out status.optimizations) > 0 )) 2>/dev/null && break
  sleep 0.2
done
ctl status >"$out"; code=$?; check "status answers" '(( code == 0 ))'
check "status: the app starts on Overview" '[[ "$(field $out status.page)" == overview ]]'
check "status: test mode is on" '[[ "$(field $out status.testMode)" == true ]]'
check "status: the daemon phase is running (sample data needs none)" '[[ "$(field $out status.daemonPhase)" == running ]]'
check "status: the sample data is loaded" '(( $(field $out status.optimizations) > 0 ))'
check "status: it names the notification authorization" '[[ -n "$(field $out status.notificationAuth)" ]]'
check "status.json is written" '[[ -s "$home/bridge/status.json" ]]'

# The self-test, inside the running app. It needs the sample data as the app started, so it goes first.
RATCHETCTL_TIMEOUT=120 ctl selftest >"$out"; code=$?
check "the self-test passes inside the running app" '(( code == 0 ))'
note "self-test: $(field $out message)"
check "the self-test ran checks" '(( $(field $out checks) > 40 ))'
(( code != 0 )) && grep -E '"FAIL' "$out" | head -20

# open: a page, then an optimization by id, then a page again.
ctl open:connectors >"$out"; check "open:connectors" '[[ "$(field $out status.page)" == connectors ]]'
ctl open:opt-checkout >"$out"
check "open:<id> selects the optimization" '[[ "$(field $out status.page)" == optimizations && "$(field $out status.selectedOptimization)" == opt-checkout ]]'
ctl open:benchmarks >"$out"; check "open:benchmarks" '[[ "$(field $out status.page)" == benchmarks && -z "$(field $out status.selectedOptimization)" ]]'
# Home and Agents are pages of the sidebar now (their screens come with the chat work).
ctl open:home >"$out"; check "open:home" '[[ "$(field $out status.page)" == home ]]'
ctl open:agents >"$out"; check "open:agents" '[[ "$(field $out status.page)" == agents ]]'

# appearance
ctl appearance:dark >"$out"; check "appearance:dark" '[[ "$(field $out status.appearance)" == dark ]]'
ctl appearance:light >"$out"; check "appearance:light" '[[ "$(field $out status.appearance)" == light ]]'

# snapshot scenes: the menu bar panel hosted in a window, an empty state, a daemon state.
for scene in menubar menubar-empty daemon-failed daemon-down loading-overview failed-connector empty-overview; do
  ctl snapshot:$scene >"$out"
  png="$home/bridge/snapshots/$scene-light.png"
  check "snapshot:$scene writes a PNG" '[[ -s "$png" ]]'
done

# A command the app does not know is answered, not ignored.
ctl nonsense >"$out"; code=$?
check "an unknown command is refused with ok=false" '(( code == 1 )) && [[ "$(field $out ok)" == false ]]'

if (( fail )); then note "FAILED"; tail -n 20 "$home/app.log"; exit 1; fi
note "PASS"
