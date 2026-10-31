#!/bin/bash
# Build-time smoke test for the bench image: fails the Packer build if what the bench runner needs
# (packages/bench remote-scripts.ts, image-scripts.ts) is missing, or if anything that must not be there is.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root
export PATH="$PREFIX/bin:$PATH"

# 1 when the image was built with -var host_toolchain=true (the legacy no-container path).
HOST_TOOLCHAIN="${RATCHET_HOST_TOOLCHAIN:-0}"
BU=ratchet-bench
HU=ratchet-holdout
LAUNCHER="$PREFIX/libexec/ratchet/ratchet-candidate-ctr"
# Must be identical to the rule in packages/aws `benchUserData` (infra/tests compares them).
IMDS_RULE=(OUTPUT -m owner --uid-owner "$BU" -d 169.254.169.254 -j REJECT)
as_bench() { setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- "$@"; }

log "commands the bench scripts use"
for c in bash tar timeout realpath date sha256sum find grep setpriv pkill chown chgrp iptables ip6tables unzip \
         zstd gzip aws git docker; do
  command -v "$c" >/dev/null || die "missing command: $c"
done
printf 'a1b2\n' | grep -oP '\d+(?=b)' | grep -qx 1 || die "grep -P does not work"
find /etc -maxdepth 0 -printf '%P\n' >/dev/null || die "find -printf does not work"
realpath -m /nonexistent/a/../b | grep -qx /nonexistent/b || die "realpath -m does not work"
date +%s%N | grep -Eq '^[0-9]{18,}$' || die "date +%s%N does not work"

log "unprivileged users"
for u in "$BU" "$HU"; do
  id -u "$u" >/dev/null || die "user $u is missing"
  [ "$(id -u "$u")" -lt 1000 ] || die "$u should be a system user"
  [ "$(getent passwd "$u" | cut -d: -f7)" = /usr/sbin/nologin ] || die "$u should have the nologin shell"
  [ ! -d "/home/$u" ] || die "$u should have no home directory"
  for g in wheel sudo docker root; do
    if in_group "$u" "$g"; then die "$u must not be in group $g"; fi
  done
  if grep -rqs "$u" /etc/sudoers /etc/sudoers.d; then die "$u appears in sudoers"; fi
done
[ -x /usr/sbin/nologin ] || die "/usr/sbin/nologin is missing"

log "docker engine"
getent group docker >/dev/null || die "the docker group is missing"
[ -z "$(getent group docker | cut -d: -f4)" ] || die "the docker group must have no members (membership is root on the box)"
systemctl is-enabled docker >/dev/null || die "docker is not enabled"
[ -x /usr/libexec/docker/docker-init ] || command -v docker-init >/dev/null || die "docker-init is missing (docker run --init)"
grep -q '"icc": false' "$ETC/docker/daemon.json" || die "daemon.json does not disable inter-container traffic"
if grep -Eq 'no-new-privileges|seccomp|"hosts"|"tcp://' "$ETC/docker/daemon.json"; then die "daemon.json must not change seccomp / no-new-privileges or open a TCP socket"; fi
systemctl start docker
for _i in $(seq 1 30); do docker info >/dev/null 2>&1 && break; sleep 2; done
docker info >/dev/null 2>&1 || die "the docker daemon did not start"
docker info --format '{{json .SecurityOptions}}' | grep -q seccomp || die "the docker daemon runs without its default seccomp profile"
[ ! -S /var/run/docker.sock ] || [ "$(stat -c %a /var/run/docker.sock)" = 660 ] || die "the docker socket must be mode 0660"
if as_bench docker info >/dev/null 2>&1; then die "$BU can talk to the docker daemon"; fi
systemctl stop docker docker.socket 2>/dev/null || true

log "candidate launcher (setuid root, static, root-only directory)"
[ "$(stat -c '%U:%G %a' "$LAUNCHER")" = "root:root 4755" ] || die "$LAUNCHER must be root:root mode 4755"
[ "$(stat -c '%U:%G %a' "$(dirname "$LAUNCHER")")" = "root:root 700" ] || die "$(dirname "$LAUNCHER") must be root:root mode 0700"
# Not `ldd`: it exits 0 ("statically linked") for a static-pie executable, which is what the Rust launcher is.
elf_is_native "$LAUNCHER" || die "$LAUNCHER is not an ELF executable for this machine"
elf_is_static "$LAUNCHER" || die "$LAUNCHER must be statically linked (it runs inside arbitrary images)"
# It runs here and refuses: root is not the caller named by a launcher.conf, and the host has none (exit 77).
launcher_rc=0
"$LAUNCHER" true >/dev/null 2>&1 || launcher_rc=$?
[ "$launcher_rc" -eq 77 ] || die "$LAUNCHER without a launcher.conf must exit 77, got $launcher_rc"
if as_bench "$LAUNCHER" true >/dev/null 2>&1; then die "$BU can execute the launcher on the host"; fi
for c in gcc cc make; do
  if command -v "$c" >/dev/null; then die "$c must not be on the bench image"; fi
done

if [ "$HOST_TOOLCHAIN" = 1 ]; then
  log "host toolchain (legacy no-container path) is runnable by $BU"
  for c in node npm pnpm k6 hyperfine; do command -v "$c" >/dev/null || die "missing command: $c"; done
  as_bench node --version | grep -qx "v${NODE_VERSION}"
  as_bench pnpm --version | grep -qx "${PNPM_VERSION}"
  as_bench k6 version | grep -q "k6 v${K6_VERSION}"
  as_bench hyperfine --version | grep -q "hyperfine ${HYPERFINE_VERSION}"
else
  log "generic image: no language toolchain on the host (the repo's container provides it)"
  for c in node npm pnpm k6 hyperfine; do
    if command -v "$c" >/dev/null; then die "$c must not be on the generic bench image"; fi
  done
fi
as_bench aws --version | grep -q "aws-cli/${AWSCLI_VERSION} "
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
