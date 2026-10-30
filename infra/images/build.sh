#!/bin/bash
# Builds one or both machine images with Packer and prints the AMI ids to pass to live mode.
#
# usage: infra/images/build.sh runner|bench|both [extra `packer build` arguments...]
#   e.g. infra/images/build.sh both -var region=us-east-1 -var subnet_id=subnet-0abc
#
# Needs: packer >= 1.10, AWS credentials for the SANDBOX account (AWS_PROFILE / SSO), pnpm + node
# (runner image only: the ratchet-runner tarballs are packed from this checkout).
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

build_one() {
  local name="$1" log
  shift
  log="$here/.last-build-$name.log"
  packer build -color=false -only="${name}.*" -var "image_version=$version" -var "source_commit=$commit" "$@" "$here" | tee "$log"
  # Packer ends with "AMIs were created:" followed by "<region>: ami-...".
  awk -v n="$name" '/AMIs were created:/ { f = 1; next } f && /^[a-z0-9-]+: ami-[0-9a-f]+/ { print n "ImageRef: " $2 }' "$log"
}

if [ "$which" = runner ] || [ "$which" = both ]; then
  "$here/pack-runner.sh"
  build_one runner "$@"
fi
if [ "$which" = bench ] || [ "$which" = both ]; then
  build_one bench "$@"
fi

echo "Pass the ami-... ids to live mode as runnerImageRef / benchImageRef (see infra/images/README.md)." >&2
