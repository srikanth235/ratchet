#!/bin/bash
# Builds one or both machine images with Packer and prints the AMI ids to pass to live mode.
#
# usage: infra/images/build.sh runner|bench|both [extra `packer build` arguments...]
#   e.g. infra/images/build.sh both -var region=us-east-1 -var subnet_id=subnet-0abc
#
# Needs: packer >= 1.10, AWS credentials for the SANDBOX account (AWS_PROFILE / SSO), pnpm + node
# (runner image only: the ratchet-runner tarballs are packed from this checkout) and, for the bench image, the prebuilt
# static launcher of the image's architecture from CI (.github/workflows/images.yml, artifact
# ratchet-candidate-launcher-<arch>): infra/images/launcher/ratchet-candidate-launcher-<arch> plus the .sha256 next to it,
# or RATCHET_LAUNCHER_BINARY=/path/to/binary (its <binary>.sha256, or RATCHET_LAUNCHER_SHA256=<hex>). <arch> is the
# Packer `arch` variable: x86_64 (default) or arm64 (pass -var arch=arm64). Nothing is compiled on the builder.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
which="${1:-}"
case "$which" in runner | bench | both) shift ;; *) echo "usage: $0 runner|bench|both [packer args...]" >&2; exit 2 ;; esac

commit="$(git -C "$repo" rev-parse --short=9 HEAD 2>/dev/null || echo unknown)"
if [ -n "$(git -C "$repo" status --porcelain 2>/dev/null)" ]; then commit="${commit}-dirty"; fi
# The tag the app matches (ratchet:image_version): the app version, then a build suffix (packages/aws findRatchetImages).
app_version="${RATCHET_APP_VERSION:-$(node -p "require('$repo/packages/desktop/package.json').version" 2>/dev/null || echo 0.0.0)}"
version="${RATCHET_IMAGE_VERSION:-${app_version}-$(date -u +%Y%m%d)-${commit}}"

packer init "$here"

# sha256_of FILE: the hex digest (sha256sum on Linux, shasum on macOS).
sha256_of() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# launcher_args ARGS...: the -var arguments for the prebuilt launcher, checked here so a wrong or missing binary fails
# before a builder instance is launched (35-candidate-launcher.sh checks the uploaded copy again on the builder).
launcher_args() {
  local arch=x86_64 prev="" a bin want got
  for a in "$@"; do
    case "$a" in arch=*) [ "$prev" = -var ] && arch="${a#arch=}" ;; -var=arch=*) arch="${a#-var=arch=}" ;; esac
    prev="$a"
  done
  bin="${RATCHET_LAUNCHER_BINARY:-$here/launcher/ratchet-candidate-launcher-$arch}"
  [ -f "$bin" ] || { echo "the prebuilt launcher $bin is missing: download the CI artifact ratchet-candidate-launcher-$arch (infra/images/README.md)" >&2; exit 2; }
  want="${RATCHET_LAUNCHER_SHA256:-}"
  if [ -z "$want" ] && [ -f "$bin.sha256" ]; then want="$(cut -d' ' -f1 < "$bin.sha256")"; fi
  [[ "$want" =~ ^[0-9a-fA-F]{64}$ ]] || { echo "no SHA-256 for $bin: keep the CI .sha256 file next to it or set RATCHET_LAUNCHER_SHA256" >&2; exit 2; }
  got="$(sha256_of "$bin")"
  want="$(printf '%s' "$want" | tr 'A-F' 'a-f')"
  [ "$got" = "$want" ] || { echo "sha256 mismatch for $bin: expected $want, got $got" >&2; exit 2; }
  printf '%s\n' -var "launcher_binary=$bin" -var "launcher_sha256=$got"
}

build_one() {
  local name="$1" log a extra=()
  shift
  if [ "$name" = bench ]; then
    # (no mapfile: macOS ships bash 3.2) a failing launcher_args leaves `extra` short and stops the build
    while IFS= read -r a; do extra+=("$a"); done < <(launcher_args "$@")
    [ "${#extra[@]}" -eq 4 ] || exit 2
  fi
  log="$here/.last-build-$name.log"
  packer build -color=false -only="${name}.*" -var "image_version=$version" -var "source_commit=$commit" ${extra[@]+"${extra[@]}"} "$@" "$here" | tee "$log"
  # Packer ends with "AMIs were created:" followed by "<region>: ami-...".
  awk -v n="$name" '/AMIs were created:/ { f = 1; next } f && /^[a-z0-9-]+: ami-[0-9a-f]+/ { print n "ImageRef: " $2 }' "$log"
}

if [ "$which" = both ]; then launcher_args "$@" >/dev/null || exit 2; fi # fail before the runner image takes its time
if [ "$which" = runner ] || [ "$which" = both ]; then
  "$here/pack-runner.sh"
  build_one runner "$@"
fi
if [ "$which" = bench ] || [ "$which" = both ]; then
  build_one bench "$@"
fi

echo "Pass the ami-... ids to live mode as runnerImageRef / benchImageRef (see infra/images/README.md)." >&2
