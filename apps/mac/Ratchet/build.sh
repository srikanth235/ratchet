#!/bin/zsh
# Builds Ratchet and installs it to ~/Applications/Ratchet.app.
# Build products live in ~/Library/Developer/Ratchet, outside the repo
# (and outside Dropbox or iCloud, which would try to sync them).
#
# Usage: ./build.sh            release build, install
#        ./build.sh --debug    debug build, install
#        ./build.sh --open     install, then quit the running copy and open the new one
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
xattr -cr "$app"
codesign --force --sign "$identity" "$app" 2>&1 | grep -v "replacing existing signature" || true
echo "installed $app (signed: ${identity})"

if [[ " $* " == *" --open "* ]]; then
  osascript -e "quit app \"$name\"" 2>/dev/null || true
  sleep 0.5
  open "$app"
fi
