#!/bin/bash
# AWS CLI v2, pinned and checksummed. The base AMI may ship another build in /usr/bin;
# $PREFIX/bin comes first in PATH, so the pinned one wins.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

zip="awscli-exe-linux-${ARCH_UNAME}-${AWSCLI_VERSION}.zip"
log "aws cli $AWSCLI_VERSION ($ARCH_UNAME)"
fetch_verified "https://awscli.amazonaws.com/${zip}" "$WORK/$zip" "$(var "AWSCLI_SHA256_${ARCH_UNAME}")"
unzip -q "$WORK/$zip" -d "$WORK/awscli"
"$WORK/awscli/aws/install" --update --install-dir "$PREFIX/aws-cli" --bin-dir "$PREFIX/bin"
make_world_readable "$PREFIX/aws-cli"
"$PREFIX/bin/aws" --version | grep -q "aws-cli/${AWSCLI_VERSION} " || die "aws reports an unexpected version"
