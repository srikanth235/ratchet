#!/bin/bash
# Runner image only: profilers and build tools the agents use while optimizing a repository.
# Not installed on the bench image, which stays minimal.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

log "dnf: profilers and build tools"
dnf_install \
  perf strace sysstat lsof bind-utils \
  gcc gcc-c++ make python3

# Let non-root processes use perf (user-space profiling, call graphs).
cat > /etc/sysctl.d/90-ratchet-perf.conf <<'SYSCTL'
kernel.perf_event_paranoid = 1
kernel.kptr_restrict = 0
SYSCTL
