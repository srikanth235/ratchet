#!/bin/bash
# Bench image only: the setuid-root launcher the evaluator harness uses INSIDE the bench container to start candidate
# processes as the unprivileged candidate uid (crates/ratchet-bench/src/launcher.rs explains the protocol). It is the Rust
# binary `ratchet-candidate-launcher`, built by CI (.github/workflows/images.yml) as a static executable per architecture and
# uploaded to the builder by Packer (bench.pkr.hcl); nothing is compiled here and the bench image has no compiler.
#
# Inputs (set by Packer; the defaults are for running this by hand):
#   RATCHET_LAUNCHER_BINARY  the prebuilt binary, default: ratchet-candidate-launcher next to this script
#   RATCHET_LAUNCHER_SHA256  its SHA-256 as published by the CI job (64 hex digits); required, a mismatch fails the build
#
# Installed in a root-only directory: on the host nobody but root can even execute it. Docker (root) bind-mounts it read-only
# into the harness container of confirm / bench runs; inside, it only works when the root-owned launcher.conf next to it names
# the calling uid (crates/ratchet-bench image_scripts.rs, the image restore fragment probes exactly that). The path of that file
# (/ratchet/launcher.conf) is compiled into the binary; nothing on the image has to provide it.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

src="${RATCHET_LAUNCHER_BINARY:-$SCRIPT_DIR/ratchet-candidate-launcher}"
want="${RATCHET_LAUNCHER_SHA256:-}"
want="${want,,}"
[ -f "$src" ] || die "the prebuilt launcher $src is missing (CI artifact ratchet-candidate-launcher-$ARCH_UNAME; see infra/images/README.md)"
[[ "$want" =~ ^[0-9a-f]{64}$ ]] || die "RATCHET_LAUNCHER_SHA256 must be the 64 hex digits of the launcher's SHA-256 (-var launcher_sha256=...)"

# Check the copy that is installed, not the upload: $WORK is a fresh 0700 directory of this (root) process.
bin="$WORK/ratchet-candidate-ctr"
cp -- "$src" "$bin"
log "verify the launcher (sha256, native static ELF)"
verify_sha256 "$bin" "$want"
elf_is_native "$bin" || die "the launcher is not a 64-bit ELF executable for this machine ($ARCH_UNAME): build the image of the architecture the binary was built for"
elf_is_static "$bin" || die "the launcher is dynamically linked: it must be static (it runs inside arbitrary images); build with RUSTFLAGS='-C target-feature=+crt-static'"
LC_ALL=C grep -qaF '/ratchet/launcher.conf' "$bin" || die "the launcher does not name /ratchet/launcher.conf: not the ratchet-candidate-launcher binary"

dest="$PREFIX/libexec/ratchet"
log "install into $dest (directory 0700 root, binary 4755 root)"
install -d -m 0700 -o root -g root "$dest"
install -m 4755 -o root -g root "$bin" "$dest/ratchet-candidate-ctr"
