#!/bin/bash
# Runs on the controller (your machine or CI), before `packer build` of the runner image.
# Builds @ratchet/core and @ratchet/agents from this checkout and packs them with `pnpm pack`
# (which rewrites the workspace dependency to a real version) into infra/images/dist/, with a
# SHA256SUMS file that scripts/60-ratchet-runner.sh verifies on the builder.
#
# usage: infra/images/pack-runner.sh [output-dir]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="${1:-$here/dist}"

cd "$repo"
pnpm install --frozen-lockfile
pnpm --filter @ratchet/core --filter @ratchet/agents build

rm -rf "$out"
mkdir -p "$out"
pnpm --filter @ratchet/core pack --pack-destination "$out"
pnpm --filter @ratchet/agents pack --pack-destination "$out"

(cd "$out" && sha256sum ratchet-core-*.tgz ratchet-agents-*.tgz > SHA256SUMS)
echo "packed:" >&2
cat "$out/SHA256SUMS" >&2
