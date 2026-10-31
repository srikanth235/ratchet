#!/bin/zsh
# CI smoke test of the real client path: starts the bundled `ratchetd serve --mode sim` on a temp socket, then runs the
# installed app's self-test against it (`--selftest-live`: the generated client, the unix socket, the token, the mapping
# and the event stream; not the sample client). Also checks the bundle's layout. Exit code 0 when everything passed.
set -u
app="${1:-$HOME/Applications/Ratchet.app}"
fail=0
note() { echo "SMOKE $*"; }

# Bundle layout.
for path in Contents/MacOS/Ratchet Contents/Helpers/ratchetd Contents/Helpers/ratchet \
            Contents/Library/LaunchAgents/com.ratchet.ratchetd.plist \
            Contents/Library/LaunchAgents/com.ratchet.ratchetd.demo.plist; do
  if [[ -e "$app/$path" ]]; then note "ok   $path"; else note "FAIL missing $path"; fail=1; fi
done
(( fail )) && exit 1
file "$app/Contents/Helpers/ratchetd"
lipo -archs "$app/Contents/Helpers/ratchetd"
plutil -p "$app/Contents/Library/LaunchAgents/com.ratchet.ratchetd.plist"
/usr/libexec/PlistBuddy -c "Print :BundleProgram" "$app/Contents/Library/LaunchAgents/com.ratchet.ratchetd.plist" | grep -qx "Contents/Helpers/ratchetd" \
  || { note "FAIL BundleProgram"; fail=1; }
/usr/libexec/PlistBuddy -c "Print :ProgramArguments" "$app/Contents/Library/LaunchAgents/com.ratchet.ratchetd.plist" | grep -q -- "--mode" \
  || { note "FAIL real plist lacks --mode"; fail=1; }
/usr/libexec/PlistBuddy -c "Print :ProgramArguments" "$app/Contents/Library/LaunchAgents/com.ratchet.ratchetd.demo.plist" | grep -qw sim \
  || { note "FAIL demo plist does not run sim"; fail=1; }
"$app/Contents/Helpers/ratchet" --help >/dev/null || { note "FAIL ratchet --help"; fail=1; }

# A short path: unix socket paths are limited to about 100 bytes.
home=$(mktemp -d /tmp/ratchet-smoke.XXXXXX)
export RATCHET_HOME="$home"
"$app/Contents/Helpers/ratchetd" serve --mode sim --ledger :memory: --tick-ms 500 >"$home/daemon.log" 2>&1 &
daemon=$!
trap 'kill $daemon 2>/dev/null; wait $daemon 2>/dev/null; rm -rf "$home"' EXIT
for _ in {1..100}; do [[ -S "$home/ratchetd.sock" && -s "$home/ratchetd.token" ]] && break; sleep 0.2; done
if [[ ! -S "$home/ratchetd.sock" ]]; then note "FAIL the daemon did not open its socket"; cat "$home/daemon.log"; exit 1; fi
note "ratchetd is listening on $home/ratchetd.sock"

# The CLI is a client of the same socket.
"$app/Contents/Helpers/ratchet" status || note "WARN ratchet status failed (not fatal)"

# The app's self-test, against the daemon.
"$app/Contents/MacOS/Ratchet" --selftest-live
code=$?
if (( code != 0 )); then
  note "FAIL the live self-test exited with $code"
  tail -n 40 "$home/daemon.log"
  exit 1
fi
(( fail )) && exit 1
note "ok   the live self-test passed"
