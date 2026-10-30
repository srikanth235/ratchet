#!/bin/bash
# Base packages shared by both images, all from the Amazon Linux 2023 repositories.
# The bench requirements (see remote-scripts.ts in packages/bench) are: bash, tar, GNU
# coreutils/findutils/grep with -P, util-linux (setpriv), procps (pkill) and sudo (the holdout user starts the
# candidate as ratchet-bench through one sudo rule, installed by the bench user data).
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

log "dnf: base packages"
# coreutils is already present as coreutils-single, which provides the same binaries.
dnf_install \
  git tar gzip xz unzip jq openssl ca-certificates \
  findutils grep util-linux procps-ng sudo \
  amazon-ssm-agent

log "SSM agent enabled at boot"
systemctl enable amazon-ssm-agent
