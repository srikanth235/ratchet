#!/bin/zsh
# Builds the Rust helpers the app bundles, `ratchetd` and `ratchet`, and writes them to the directory given.
#
# Usage: tools/build-helpers.sh <out-dir> [--universal]
#   default      the Mac's own architecture (fast, for the dev loop)
#   --universal  arm64 and x86_64, joined with lipo (CI and releases)
#
# Needs a Rust toolchain (rustup.rs). Cargo's target directory is the workspace's own (<repo>/target), so reruns are
# incremental and CI can cache it.
set -euo pipefail
here=${0:A:h}
root=${here:h:h:h:h}
out=${1:?usage: build-helpers.sh <out-dir> [--universal]}
universal=0
[[ " $* " == *" --universal "* || -n "${RATCHET_UNIVERSAL:-}" ]] && universal=1

command -v cargo >/dev/null || { echo "build-helpers: cargo not found (install Rust from https://rustup.rs)"; exit 1; }

if (( universal )); then
  triples=(aarch64-apple-darwin x86_64-apple-darwin)
elif [[ "$(uname -m)" == arm64 ]]; then
  triples=(aarch64-apple-darwin)
else
  triples=(x86_64-apple-darwin)
fi

mkdir -p "$out"
for triple in $triples; do
  command -v rustup >/dev/null && rustup target add "$triple" >/dev/null 2>&1 || true
  echo "build-helpers: cargo build --release --target $triple"
  cargo build --release --manifest-path "$root/Cargo.toml" --target "$triple" -p ratchetd -p ratchet-cli
done

for bin in ratchetd ratchet; do
  inputs=()
  for triple in $triples; do inputs+=("$root/target/$triple/release/$bin"); done
  if (( ${#inputs} == 1 )); then
    cp "${inputs[1]}" "$out/$bin"
  else
    lipo -create -output "$out/$bin" $inputs
  fi
  chmod 755 "$out/$bin"
  echo "build-helpers: $out/$bin ($(lipo -archs "$out/$bin"))"
done
