#!/bin/bash
# Runner image only: the two agent CLIs, version-pinned. The top-level npm tarball of each is
# checked against the registry SHA-512 recorded in versions-agents.sh before it is installed; the
# platform binaries they pull in (optionalDependencies at the same exact version) are
# verified by npm against the registry metadata.
#
# This script must never run for the bench image: no agent CLIs and no model keys there.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
# shellcheck source=versions-agents.sh
. "$SCRIPT_DIR/versions-agents.sh"
require_root
export PATH="$PREFIX/bin:$PATH"

log "claude code $CLAUDE_CODE_VERSION"
claude_tgz="$(npm_fetch_verified @anthropic-ai/claude-code "$CLAUDE_CODE_VERSION" "$CLAUDE_CODE_INTEGRITY")"
npm install --global --prefix "$PREFIX" --no-audit --no-fund "$claude_tgz"

log "codex $CODEX_VERSION"
codex_tgz="$(npm_fetch_verified @openai/codex "$CODEX_VERSION" "$CODEX_INTEGRITY")"
npm install --global --prefix "$PREFIX" --no-audit --no-fund "$codex_tgz"

make_world_readable "$PREFIX/lib/node_modules"

# The pinned version must stay pinned: the CLI must not replace itself at run time.
# /etc/claude-code/managed-settings.json is Claude Code's machine-wide settings file on Linux.
install -d -m 0755 "$ETC/claude-code"
cat > "$ETC/claude-code/managed-settings.json" <<'JSON'
{
  "env": {
    "DISABLE_AUTOUPDATER": "1"
  }
}
JSON
chmod 0644 "$ETC/claude-code/managed-settings.json"

claude --version | grep -q "$CLAUDE_CODE_VERSION" || die "claude reports an unexpected version"
codex --version | grep -q "$CODEX_VERSION" || die "codex reports an unexpected version"
