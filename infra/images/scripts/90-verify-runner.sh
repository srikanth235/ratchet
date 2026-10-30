#!/bin/bash
# Build-time smoke test for the runner image: fails the Packer build if something the agent
# runner needs is missing or at the wrong version. No model keys are needed (or allowed) here.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
# shellcheck source=versions-agents.sh
. "$SCRIPT_DIR/versions-agents.sh"
require_root
export PATH="$PREFIX/bin:$PATH"

log "commands"
for c in node npm pnpm git aws claude codex ratchet-runner ratchet k6 hyperfine perf jq unzip tar make gcc; do
  command -v "$c" >/dev/null || die "missing command: $c"
done

log "versions"
[ "$(node --version)" = "v${NODE_VERSION}" ] || die "unexpected node version"
[ "$(pnpm --version)" = "${PNPM_VERSION}" ] || die "unexpected pnpm version"
aws --version | grep -q "aws-cli/${AWSCLI_VERSION} " || die "unexpected aws cli version"
k6 version | grep -q "k6 v${K6_VERSION}" || die "unexpected k6 version"
hyperfine --version | grep -q "hyperfine ${HYPERFINE_VERSION}" || die "unexpected hyperfine version"
claude --version | grep -q "${CLAUDE_CODE_VERSION}" || die "unexpected claude version"
codex --version | grep -q "${CODEX_VERSION}" || die "unexpected codex version"

log "ratchet-runner loads and validates its environment (exit 2 = bad configuration)"
set +e
out="$(env -i PATH="$PATH" ratchet-runner 2>&1)"
rc=$?
set -e
[ "$rc" -eq 2 ] || die "ratchet-runner without configuration should exit 2, got $rc: $out"
case "$out" in *RATCHET_OPTIMIZATION_ID*) ;; *) die "unexpected ratchet-runner output: $out" ;; esac

log "SSM agent"
systemctl is-enabled amazon-ssm-agent >/dev/null || die "amazon-ssm-agent is not enabled"

log "no model keys or launch state baked in"
for v in $(compgen -e); do
  case "$v" in ANTHROPIC_* | OPENAI_* | CODEX_*) die "model key variable $v is set" ;; esac
done
[ ! -e /etc/ratchet/runner.env ] || die "/etc/ratchet/runner.env must come from user data, not the image"
[ ! -e "$HOME/.claude" ] && [ ! -e "$HOME/.codex" ] || die "agent CLI state found in $HOME"

log "runner image OK"
