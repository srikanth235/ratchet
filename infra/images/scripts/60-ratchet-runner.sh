#!/bin/bash
# Runner image only: install the ratchet-runner bin from this repository's prebuilt static binary.
#
# Input: a directory built on the controller by infra/images/pack-runner.sh, containing
#   ratchet-runner  SHA256SUMS
# (the musl-static Rust binary of crates/ratchet-agents; CI artifact ratchet-runner-<arch>). Nothing is compiled here and
# no Node package is installed for it.
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root
export PATH="$PREFIX/bin:$PATH"

dist="${RATCHET_RUNNER_DIST:-/tmp/ratchet-image/dist}"
app="$PREFIX/lib/ratchet-runner"

[ -f "$dist/SHA256SUMS" ] || die "$dist/SHA256SUMS is missing (run infra/images/pack-runner.sh before packer build)"
(cd "$dist" && sha256sum --check --strict SHA256SUMS) || die "the runner binary does not match SHA256SUMS"
grep -q " ratchet-runner\$" "$dist/SHA256SUMS" || die "ratchet-runner is not listed in SHA256SUMS"
[ -f "$dist/ratchet-runner" ] || die "$dist/ratchet-runner is missing"

log "ratchet-runner from $dist/ratchet-runner"
rm -rf "$app"
mkdir -p "$app"
install -m 0755 "$dist/ratchet-runner" "$app/ratchet-runner"
make_world_readable "$app"
# it must run on this builder (right architecture, really static)
"$app/ratchet-runner" --version >/dev/null || die "the ratchet-runner binary does not run on this machine (wrong architecture?)"

# Launcher on PATH. SsmRunnerControl starts `ratchet-runner` from a plain SSM shell, which does not
# have the launch environment that the instance user data wrote to /etc/ratchet/runner.env
# (RATCHET_SECRET_REFS, proxy settings). The launcher loads that file, but anything the caller
# already exported (ids, artifacts bucket, region) wins.
mkdir -p "$PREFIX/bin"
cat > "$PREFIX/bin/ratchet-runner" <<'LAUNCHER'
#!/bin/bash
set -euo pipefail
export PATH="@PREFIX@/bin:$PATH"
env_file=/etc/ratchet/runner.env
if [ -r "$env_file" ]; then
  saved="$(export -p)"
  set -a
  # shellcheck disable=SC1090
  . "$env_file"
  set +a
  eval "$saved" 2>/dev/null || true
fi
exec "@APP@/ratchet-runner" "$@"
LAUNCHER
sed -i "s#@PREFIX@#$PREFIX#g; s#@APP@#$app#g" "$PREFIX/bin/ratchet-runner"
chmod 0755 "$PREFIX/bin/ratchet-runner"
