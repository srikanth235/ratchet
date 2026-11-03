#!/bin/bash
# Load generator and benchmarking tools shared by both images: k6 and hyperfine, from the
# projects' release tarballs, SHA-256 checked, installed world-readable.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

mkdir -p "$PREFIX/bin"

k6_dir="k6-v${K6_VERSION}-linux-${ARCH_GO}"
log "k6 $K6_VERSION ($ARCH_GO)"
fetch_verified "https://github.com/grafana/k6/releases/download/v${K6_VERSION}/${k6_dir}.tar.gz" "$WORK/k6.tgz" "$(var "K6_SHA256_${ARCH_GO}")"
tar -xzf "$WORK/k6.tgz" -C "$WORK" --no-same-owner
install -m 0755 "$WORK/$k6_dir/k6" "$PREFIX/bin/k6"

hf_dir="hyperfine-v${HYPERFINE_VERSION}-${ARCH_UNAME}-unknown-linux-gnu"
log "hyperfine $HYPERFINE_VERSION ($ARCH_UNAME)"
fetch_verified "https://github.com/sharkdp/hyperfine/releases/download/v${HYPERFINE_VERSION}/${hf_dir}.tar.gz" "$WORK/hyperfine.tgz" "$(var "HYPERFINE_SHA256_${ARCH_UNAME}")"
tar -xzf "$WORK/hyperfine.tgz" -C "$WORK" --no-same-owner
install -m 0755 "$WORK/$hf_dir/hyperfine" "$PREFIX/bin/hyperfine"

"$PREFIX/bin/k6" version | grep -q "k6 v${K6_VERSION}" || die "k6 reports an unexpected version"
"$PREFIX/bin/hyperfine" --version | grep -q "hyperfine ${HYPERFINE_VERSION}" || die "hyperfine reports an unexpected version"
