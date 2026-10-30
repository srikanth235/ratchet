#!/bin/bash
# Bench image only: the two unprivileged OS users of a bench job and the firewall tooling the
# instance user data needs. ratchet-bench runs candidate code; ratchet-holdout runs the trusted
# evaluator harness and owns the holdout file (its one privilege, a sudo rule that starts the
# candidate as ratchet-bench, is installed by the user data, not here).
#
# What lives where (kept in sync with packages/aws/src/compute/aws.ts `benchUserData`, which is
# checked by infra/tests):
#   - the image (this script) creates both users, with the very same useradd flags, and installs
#     iptables/ip6tables, without which the user data's `set -e` script would abort;
#   - the user data adds the IMDS block (`iptables -I OUTPUT 1 -m owner --uid-owner <user>
#     -d 169.254.169.254 -j REJECT` for each user, plus the IPv6 twin) on every boot. The image must NOT install
#     that rule itself or persist rules (no iptables-services): a second copy would hide a broken
#     user data, and the bench runner refuses to run a job unless the rule is present.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

BENCH_USER=ratchet-bench
HOLDOUT_USER=ratchet-holdout

log "dnf: iptables (nft backend, ships ip6tables too)"
dnf_install iptables-nft

for u in "$BENCH_USER" "$HOLDOUT_USER"; do
  log "user $u"
  id -u "$u" >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin "$u"
  # No sudo group, no supplementary groups, no password.
  for g in wheel sudo adm systemd-journal; do
    if in_group "$u" "$g"; then die "$u must not be in group $g"; fi
  done
  passwd -l "$u" >/dev/null 2>&1 || true
done
