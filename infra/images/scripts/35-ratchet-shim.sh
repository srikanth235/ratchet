#!/bin/bash
# Both images: a `ratchet` command that the instance user data (packages/aws `runnerUserData` /
# `benchUserData`) ends with: `nohup ratchet runner ...` and `nohup ratchet bench-agent ...`.
#
# Neither is a daemon in this design. The app starts the agent loop itself over SSM
# (SsmRunnerControl runs `ratchet-runner`) and drives the bench with generated scripts through
# SSM Run Command (RemoteBenchRunner), so a second runner started from user data would race the
# SSM-started one. The shim therefore only acknowledges the call; it never starts a loop.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root

mkdir -p "$PREFIX/bin"
cat > "$PREFIX/bin/ratchet" <<'SHIM'
#!/bin/sh
case "${1:-}" in
  runner | bench-agent)
    echo "ratchet $1: nothing to do at boot; the app drives this machine over SSM (see infra/images/README.md)."
    exit 0
    ;;
  *)
    echo "usage: ratchet runner | bench-agent (boot-time no-ops; the loop is ratchet-runner on the runner image)" >&2
    exit 2
    ;;
esac
SHIM
chmod 0755 "$PREFIX/bin/ratchet"
