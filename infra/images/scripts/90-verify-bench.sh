#!/bin/bash
# Build-time smoke test for the bench image: fails the Packer build if what the bench runner needs
# (packages/bench remote-scripts.ts) is missing, or if anything that must not be there is.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root
export PATH="$PREFIX/bin:$PATH"

BU=ratchet-bench
# Must be identical to the rule in packages/aws `benchUserData` (infra/tests compares them).
IMDS_RULE=(OUTPUT -m owner --uid-owner "$BU" -d 169.254.169.254 -j REJECT)
as_bench() { setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- "$@"; }

log "commands the bench scripts use"
for c in bash tar timeout realpath date sha256sum find grep setpriv pkill chown chgrp iptables ip6tables unzip \
         aws node npm pnpm git k6 hyperfine; do
  command -v "$c" >/dev/null || die "missing command: $c"
done
printf 'a1b2\n' | grep -oP '\d+(?=b)' | grep -qx 1 || die "grep -P does not work"
find /etc -maxdepth 0 -printf '%P\n' >/dev/null || die "find -printf does not work"
realpath -m /nonexistent/a/../b | grep -qx /nonexistent/b || die "realpath -m does not work"
date +%s%N | grep -Eq '^[0-9]{18,}$' || die "date +%s%N does not work"

log "unprivileged user"
id -u "$BU" >/dev/null || die "user $BU is missing"
[ "$(id -u "$BU")" -lt 1000 ] || die "$BU should be a system user"
[ "$(getent passwd "$BU" | cut -d: -f7)" = /usr/sbin/nologin ] || die "$BU should have the nologin shell"
[ -x /usr/sbin/nologin ] || die "/usr/sbin/nologin is missing"
[ ! -d "/home/$BU" ] || die "$BU should have no home directory"
for g in wheel sudo; do
  if in_group "$BU" "$g"; then die "$BU must not be in group $g"; fi
done
if grep -rqs "$BU" /etc/sudoers /etc/sudoers.d; then die "$BU appears in sudoers"; fi

log "toolchain is runnable by $BU"
as_bench node --version | grep -qx "v${NODE_VERSION}"
as_bench pnpm --version | grep -qx "${PNPM_VERSION}"
as_bench aws --version | grep -q "aws-cli/${AWSCLI_VERSION} "
as_bench k6 version | grep -q "k6 v${K6_VERSION}"
as_bench hyperfine --version | grep -q "hyperfine ${HYPERFINE_VERSION}"
as_bench git --version >/dev/null

log "no agent CLIs, no model keys"
for c in claude codex ratchet-runner; do
  if command -v "$c" >/dev/null; then die "$c must not be on the bench image"; fi
done
if [ -d "$PREFIX/lib/node_modules" ] && [ -n "$(find "$PREFIX/lib/node_modules" -maxdepth 2 \( -iname '*claude*' -o -iname '*codex*' -o -iname '*anthropic*' -o -iname '*openai*' \))" ]; then
  die "agent packages found on the bench image"
fi
for v in $(compgen -e); do
  case "$v" in ANTHROPIC_* | OPENAI_* | CODEX_*) die "model key variable $v is set" ;; esac
done

log "SSM agent"
systemctl is-enabled amazon-ssm-agent >/dev/null || die "amazon-ssm-agent is not enabled"

log "the kernel supports the owner match the user data relies on (rule added, probed, removed)"
cleanup() { iptables -D "${IMDS_RULE[@]}" 2>/dev/null || true; rm -rf "$WORK"; }
trap cleanup EXIT
iptables -I "${IMDS_RULE[@]}"
iptables -C "${IMDS_RULE[@]}" || die "owner rule was not accepted by iptables"
if as_bench timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then
  die "instance metadata is reachable as $BU even with the owner rule"
fi
iptables -D "${IMDS_RULE[@]}"
if iptables -C "${IMDS_RULE[@]}" 2>/dev/null; then die "the image must not keep the IMDS rule; the user data adds it"; fi

log "bench image OK"
