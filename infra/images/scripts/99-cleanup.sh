#!/bin/bash
# Last step of both builds: leave nothing machine-specific, secret or cache-sized in the AMI.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

log "package caches"
dnf clean all
rm -rf /var/cache/dnf /root/.npm /home/*/.npm /root/.cache /home/*/.cache

log "logs and cloud-init state (instances must run user data again)"
cloud-init clean --logs || true
find /var/log -type f -exec truncate -s 0 {} + 2>/dev/null || true

log "build-time ssh keys and shell history"
rm -f /root/.ssh/authorized_keys /home/*/.ssh/authorized_keys
rm -f /root/.bash_history /home/*/.bash_history

log "uploaded build inputs"
rm -rf /tmp/ratchet-image
sync
