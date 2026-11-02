#!/bin/zsh
# Builds Ratchet and installs it to ~/Applications/Ratchet.app.
# Build products live in ~/Library/Developer/Ratchet, outside the repo
# (and outside Dropbox or iCloud, which would try to sync them).
#
# Usage: ./build.sh            release build, install
#        ./build.sh --debug    debug build, install
#        ./build.sh --open     quit the running copy and stop its daemon (launchctl bootout) before the bundle is replaced,
#                              install, then open the new one. The app restarts the daemon when its version differs.
#        ./build.sh --universal  also build ratchetd and ratchet for arm64 and x86_64 (CI; the default is this Mac's arch)
#
# The app bundles the Rust daemon: Contents/Helpers/ratchetd and ratchet, and the LaunchAgent plists the app registers
# with SMAppService in Contents/Library/LaunchAgents/. RATCHET_SKIP_DAEMON=1 leaves them out (a quick UI-only build);
# RATCHET_REQUIRE_DAEMON=1 makes a failed helper build fail the script (CI). A build without the daemon says so loudly at
# the end, and the app it makes shows "This build has no background service" instead of waiting for one.
set -euo pipefail
here=${0:A:h}
name="Ratchet"
scratch="$HOME/Library/Developer/$name/build"
config=release
[[ " $* " == *" --debug "* ]] && config=debug

# Sign with your Apple Development certificate when you have one (Xcode >
# Settings > Accounts). Otherwise sign ad hoc, which is fine for your own Mac.
identity="${SIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  identity=$(security find-identity -v -p codesigning 2>/dev/null | grep -m1 -o '"Apple Development: [^"]*"' | tr -d '"' || true)
fi
[[ -n "$identity" ]] || identity=-

log=$(mktemp)
if swift build -c $config --package-path "$here" --scratch-path "$scratch" >"$log" 2>&1; then
  grep -E "error|warning|Build complete" "$log" | grep -v "^\[" || true
else
  # Full compiler output with context, so a CI log shows every error and where it is.
  echo "swift build failed:"
  grep -E "error:" -B1 -A6 "$log" | head -400 || cat "$log"
  exit 1
fi
rm -f "$log"
bin="$scratch/$config/$name"
[[ -x "$bin" ]] || { echo "build failed"; exit 1; }
# swift build can report success while an edited file failed to compile. Catch that.
stale=$(find "$here/Sources" -name '*.swift' -newer "$bin" | head -1)
if [[ -n "$stale" ]]; then
  echo "build failed: binary is older than $stale"
  swift build -c $config --package-path "$here" --scratch-path "$scratch" 2>&1 | grep -E "error" | head -20
  exit 1
fi

app="$HOME/Applications/$name.app"
mkdir -p "$HOME/Applications"
# The running copy and its daemon belong to the bundle that is about to be replaced. Quit the app, and stop the
# LaunchAgent: a daemon still running from the old bundle would answer the new app with the old code. The new app
# registers the agent again when it starts (and restarts it when the daemon's version differs).
if [[ " $* " == *" --open "* ]]; then
  osascript -e "quit app \"$name\"" 2>/dev/null || true
  for label in com.ratchet.ratchetd com.ratchet.ratchetd.demo; do
    launchctl bootout "gui/$UID/$label" 2>/dev/null || true
  done
  sleep 0.5
fi
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/$name"
sed -e "s/__BUILD__/$(date +%Y%m%d%H%M)/" "$here/Info.plist.in" > "$app/Contents/Info.plist"
# The icon: Icon/AppIcon.icon -> Resources/Assets.car + AppIcon.icns, rebuilt when it changed.
if [[ -d "$here/Icon/AppIcon.icon" ]]; then
  car="$here/Resources/Assets.car"
  if [[ ! -f "$car" || -n "$(find "$here/Icon/AppIcon.icon" -newer "$car" | head -1)" ]]; then
    # An icon problem must not stop the build: the app runs without it.
    "$here/tools/make_icon.sh" || echo "warning: the icon was not built"
  fi
  if [[ -f "$car" ]]; then cp -X "$car" "$here/Resources/AppIcon.icns" "$app/Contents/Resources/" || true; fi
fi
# The design's fonts (Instrument Serif and Sans, SIL OFL): Resources/Fonts, found by Info.plist's ATSApplicationFontsPath
# and registered again at launch (DesignKit/Fonts.swift).
if [[ -d "$here/Fonts" ]]; then
  mkdir -p "$app/Contents/Resources/Fonts"
  cp -X "$here"/Fonts/* "$app/Contents/Resources/Fonts/"
fi
# The daemon and the command line tool, and the LaunchAgent plists that run the daemon (real, and demo on the simulation).
bundled_daemon=yes
if [[ -n "${RATCHET_SKIP_DAEMON:-}" ]]; then
  bundled_daemon="no (RATCHET_SKIP_DAEMON is set)"
else
  helpers="$app/Contents/Helpers"
  agents="$app/Contents/Library/LaunchAgents"
  flags=()
  [[ " $* " == *" --universal "* ]] && flags+=(--universal)
  if "$here/tools/build-helpers.sh" "$helpers" $flags; then
    mkdir -p "$agents"
    for demo in "" "--demo"; do
      plist="$agents/com.ratchet.ratchetd${demo:+.demo}.plist"
      "$helpers/ratchetd" launchagent print --bundle-program Contents/Helpers/ratchetd \
        --associated-bundle-id dev.ratchet.app $demo > "$plist"
      plutil -lint "$plist" >/dev/null
    done
    # The OpenTofu module real runs apply (the Electron app shipped it as extraResources). ratchetd finds it from where
    # it lives: Contents/Helpers/ratchetd -> Contents/Resources/infra/aws.
    infra="$here/../../../infra/aws"
    if [[ -d "$infra" ]]; then
      mkdir -p "$app/Contents/Resources/infra"
      rm -rf "$app/Contents/Resources/infra/aws"
      rsync -a --exclude tests --exclude .terraform --exclude '*.tfstate*' "$infra/" "$app/Contents/Resources/infra/aws/"
    fi
    for tool in ratchetd ratchet; do
      codesign --force --sign "$identity" --identifier "com.ratchet.$tool" "$helpers/$tool" 2>&1 \
        | grep -v "replacing existing signature" || true
    done
  elif [[ -n "${RATCHET_REQUIRE_DAEMON:-}" ]]; then
    echo "build failed: the daemon could not be built"
    exit 1
  else
    rm -rf "$helpers"
    bundled_daemon="no (the Rust helpers failed to build; is a Rust toolchain installed?)"
  fi
fi
# `xattr -cr` is not understood by every macOS: clear each file's attributes instead.
find "$app" -exec xattr -c {} + 2>/dev/null || true
codesign --force --sign "$identity" "$app" 2>&1 | grep -v "replacing existing signature" || true
echo "installed $app (signed: ${identity})"

if [[ "$bundled_daemon" != yes ]]; then
  # Loud on purpose: a copy without ratchetd cannot run optimizations, and a quiet warning scrolls away.
  echo ""
  echo "######################################################################"
  echo "# WARNING: this build has NO bundled daemon: $bundled_daemon"
  echo "# $app can open, but shows \"This build has no background service\""
  echo "# and runs nothing. Fix the cause and run ./build.sh again, or start"
  echo "# a daemon yourself and open the app with RATCHET_HOME=<its folder>."
  echo "######################################################################"
  echo ""
fi

if [[ " $* " == *" --open "* ]]; then
  open "$app"
fi
