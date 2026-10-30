#!/bin/bash
# Runner image only: install the ratchet-runner bin from this repository's packed tarballs.
#
# Input: a directory built on the controller by infra/images/pack-runner.sh, containing
#   ratchet-core-<v>.tgz  ratchet-agents-<v>.tgz  SHA256SUMS
# (`pnpm pack` output; @ratchet/core is a workspace dependency of @ratchet/agents, so both are
# installed together, with the core dependency forced to the local tarball.)
set -euo pipefail
# shellcheck source=lib.sh
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
require_root
export PATH="$PREFIX/bin:$PATH"

dist="${RATCHET_RUNNER_DIST:-/tmp/ratchet-image/dist}"
app="$PREFIX/lib/ratchet-runner"

[ -f "$dist/SHA256SUMS" ] || die "$dist/SHA256SUMS is missing (run infra/images/pack-runner.sh before packer build)"
(cd "$dist" && sha256sum --check --strict SHA256SUMS) || die "runner tarballs do not match SHA256SUMS"
core_tgz="$(cd "$dist" && ls ratchet-core-*.tgz)"
agents_tgz="$(cd "$dist" && ls ratchet-agents-*.tgz)"
[ -n "$core_tgz" ] && [ -n "$agents_tgz" ] || die "expected one ratchet-core and one ratchet-agents tarball in $dist"
grep -q " $core_tgz\$" "$dist/SHA256SUMS" || die "$core_tgz is not listed in SHA256SUMS"
grep -q " $agents_tgz\$" "$dist/SHA256SUMS" || die "$agents_tgz is not listed in SHA256SUMS"

log "ratchet-runner from $agents_tgz + $core_tgz"
rm -rf "$app"
mkdir -p "$app/tarballs"
cp "$dist/$core_tgz" "$dist/$agents_tgz" "$app/tarballs/"
cat > "$app/package.json" <<JSON
{
  "name": "ratchet-runner-image",
  "private": true,
  "dependencies": {
    "@ratchet/agents": "file:./tarballs/$agents_tgz",
    "@ratchet/core": "file:./tarballs/$core_tgz"
  },
  "overrides": {
    "@ratchet/core": "file:./tarballs/$core_tgz"
  }
}
JSON
(cd "$app" && npm install --omit=dev --ignore-scripts --no-audit --no-fund)
make_world_readable "$app"
[ -x "$app/node_modules/.bin/ratchet-runner" ] || die "npm did not link the ratchet-runner bin"

# Launcher on PATH. SsmRunnerControl starts `ratchet-runner` from a plain SSM shell, which does not
# have the launch environment that the instance user data wrote to /etc/ratchet/runner.env
# (RATCHET_SECRET_REFS, proxy settings). The launcher loads that file, but anything the caller
# already exported (ids, artifacts bucket, region) wins.
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
exec "@APP@/node_modules/.bin/ratchet-runner" "$@"
LAUNCHER
sed -i "s#@PREFIX@#$PREFIX#g; s#@APP@#$app#g" "$PREFIX/bin/ratchet-runner"
chmod 0755 "$PREFIX/bin/ratchet-runner"
