#!/bin/bash
# Bench image only: the unprivileged OS user that runs candidate code, and the firewall tooling
# the instance user data needs.
#
# What lives where (kept in sync with packages/aws/src/compute/aws.ts `benchUserData`, which is
# checked by infra/tests):
#   - the image (this script) creates the user, with the very same useradd flags, and installs
#     iptables/ip6tables, without which the user data's `set -e` script would abort;
#   - the user data adds the IMDS block (`iptables -I OUTPUT 1 -m owner --uid-owner ratchet-bench
#     -d 169.254.169.254 -j REJECT`, plus the IPv6 twin) on every boot. The image must NOT install
#     that rule itself or persist rules (no iptables-services): a second copy would hide a broken
#     user data, and the bench runner refuses to run a job unless the rule is present.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

BENCH_USER=ratchet-bench

log "dnf: iptables (nft backend, ships ip6tables too)"
dnf_install iptables-nft

log "user $BENCH_USER"
id -u "$BENCH_USER" >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin "$BENCH_USER"
# No sudo, no supplementary groups, no password.
for g in wheel sudo adm systemd-journal; do
  if in_group "$BENCH_USER" "$g"; then die "$BENCH_USER must not be in group $g"; fi
done
passwd -l "$BENCH_USER" >/dev/null 2>&1 || true
