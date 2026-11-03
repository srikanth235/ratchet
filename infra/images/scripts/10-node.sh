#!/bin/bash
# Node.js 22 (official tarball, SHA-256 checked) and pnpm (npm tarball, SRI checked).
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

node_dir="$PREFIX/lib/nodejs"
tarball="node-v${NODE_VERSION}-linux-${ARCH_NODE}.tar.xz"
log "node $NODE_VERSION ($ARCH_NODE)"
fetch_verified "https://nodejs.org/dist/v${NODE_VERSION}/${tarball}" "$WORK/$tarball" "$(var "NODE_SHA256_${ARCH_NODE}")"
rm -rf "$node_dir"
mkdir -p "$node_dir"
tar -xJf "$WORK/$tarball" -C "$node_dir" --strip-components=1 --no-same-owner
make_world_readable "$node_dir"
for b in node npm npx; do link_into_bin "$node_dir/bin/$b" "$b"; done
export PATH="$PREFIX/bin:$PATH"
[ "$(node -p 'process.versions.node')" = "$NODE_VERSION" ] || die "node reports an unexpected version"

log "pnpm $PNPM_VERSION"
pnpm_tgz="$(npm_fetch_verified pnpm "$PNPM_VERSION" "$PNPM_INTEGRITY")"
npm install --global --prefix "$PREFIX" --no-audit --no-fund --ignore-scripts "$pnpm_tgz"
[ "$(pnpm --version)" = "$PNPM_VERSION" ] || die "pnpm reports an unexpected version"
make_world_readable "$PREFIX/lib/node_modules"
