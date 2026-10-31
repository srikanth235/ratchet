#!/bin/bash
# Runs on the controller (your machine or CI), before `packer build` of the runner image.
# Puts the static Rust `ratchet-runner` (crates/ratchet-agents, bin ratchet-runner) into infra/images/dist/ with a SHA256SUMS
# file that scripts/60-ratchet-runner.sh verifies on the builder. Nothing is compiled on the builder.
#
# The binary is the musl-static build of the image's architecture, so it runs on any Linux the AMI is based on:
#   - from CI: download the artifact `ratchet-runner-<arch>` of .github/workflows/images.yml and pass it with
#     RATCHET_RUNNER_BINARY=/path/to/ratchet-runner-<arch> (its .sha256 next to it is checked when present);
#   - or build it here: needs rustup, a musl C compiler (musl-gcc) and the target of the image's architecture.
#
# usage: [RATCHET_RUNNER_BINARY=...] infra/images/pack-runner.sh [x86_64|arm64] [output-dir]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
arch="${1:-x86_64}"
out="${2:-$here/dist}"
case "$arch" in
  x86_64) target=x86_64-unknown-linux-musl ;;
  arm64) target=aarch64-unknown-linux-musl ;;
  *) echo "usage: $0 [x86_64|arm64] [output-dir]" >&2; exit 2 ;;
esac

bin="${RATCHET_RUNNER_BINARY:-}"
if [ -z "$bin" ]; then
  (cd "$repo" && RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static" CARGO_PROFILE_RELEASE_STRIP=symbols \
    cargo build --release -p ratchet-agents --bin ratchet-runner --target "$target")
  bin="$repo/target/$target/release/ratchet-runner"
elif [ -f "$bin.sha256" ]; then
  want="$(cut -d' ' -f1 "$bin.sha256")"
  got="$(sha256sum "$bin" | cut -d' ' -f1)"
  [ "$want" = "$got" ] || { echo "$bin does not match $bin.sha256" >&2; exit 1; }
fi
[ -f "$bin" ] || { echo "no runner binary at $bin" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out"
cp "$bin" "$out/ratchet-runner"
chmod 0755 "$out/ratchet-runner"
(cd "$out" && sha256sum ratchet-runner > SHA256SUMS)
echo "packed:" >&2
cat "$out/SHA256SUMS" >&2
