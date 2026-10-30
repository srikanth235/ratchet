#!/bin/bash
# Bench image only: the setuid-root launcher the evaluator harness uses INSIDE the bench container to start candidate
# processes as the unprivileged candidate uid (ratchet-candidate-ctr.c explains the protocol). It is compiled here, statically,
# so the same binary works in any repo image of this architecture, and the compiler is removed again: the bench image has no
# compilers.
#
# Installed in a root-only directory: on the host nobody but root can even execute it. Docker (root) bind-mounts it read-only
# into the harness container of confirm / bench runs; inside, it only works when the root-owned launcher.conf next to it names
# the calling uid (packages/bench image-scripts.ts, imageRestoreFragment probes exactly that).
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

src="$SCRIPT_DIR/ratchet-candidate-ctr.c"
[ -f "$src" ] || die "ratchet-candidate-ctr.c is missing next to the scripts"

log "dnf: gcc + static glibc (removed again below)"
dnf_install gcc glibc-static

log "compile the launcher"
gcc -O2 -static -s -Wall -Wextra -Werror -o "$WORK/ratchet-candidate-ctr" "$src"

dest="$PREFIX/libexec/ratchet"
log "install into $dest (directory 0700 root, binary 4755 root)"
install -d -m 0700 -o root -g root "$dest"
install -m 4755 -o root -g root "$WORK/ratchet-candidate-ctr" "$dest/ratchet-candidate-ctr"

log "remove the compiler"
dnf remove -y gcc glibc-static
