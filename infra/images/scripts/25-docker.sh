#!/bin/bash
# Bench image only: the Docker engine. The repo's container (`environment.image`) is built by the dependency/image job on
# an instance of this image that has egress, saved to the artifacts bucket and loaded (`docker load`) on the offline bench
# instances, where setup, guardrails and the evaluator run inside it with `--network none` (packages/bench image-scripts.ts).
#
# What this step does NOT do, on purpose:
#   - add any user to the `docker` group (membership is root on the box): the candidate and holdout users are created
#     without supplementary groups and the bench checkout refuses to run when they are in docker/root/wheel/sudo;
#   - turn on daemon-wide `no-new-privileges` (the evaluator's launcher is setuid inside its container), disable seccomp, or
#     enable the TCP socket / userland proxy. The default seccomp profile stays; the checkout verifies it is active.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

log "dnf: docker (Amazon Linux 2023 repository)"
dnf_install docker

log "daemon configuration"
install -d -m 0755 "$ETC/docker"
cat > "$ETC/docker/daemon.json" <<'EOF'
{
  "icc": false,
  "userland-proxy": false,
  "live-restore": false,
  "log-driver": "local",
  "log-opts": { "max-size": "10m", "max-file": "2" },
  "default-ulimits": { "nofile": { "Name": "nofile", "Hard": 1048576, "Soft": 1048576 } }
}
EOF
chmod 0644 "$ETC/docker/daemon.json"

log "docker enabled at boot"
systemctl enable docker
