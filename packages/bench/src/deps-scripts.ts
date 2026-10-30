import { DEPS_PREFIX, DEPS_USER, type DepsStrategy } from './deps.js';
import { BenchError } from './errors.js';
import { assertContainerRuntime, containerRun, ctrFunction, ensureImageLoaded, type ImageRestore } from './image-scripts.js';
import { assertSafeRef, sq, SENTINEL, type Isolation, type Layout, type StagedArchive } from './remote-scripts.js';

/**
 * The dependency-snapshot job: ONE script, run through `CloudCompute.runCommand` on an instance launched with purpose `deps`
 * (agent subnet, egress through the proxy when enabled, an identity that can write `bench-deps/` and nothing else).
 *
 * What it does, in order:
 *   1. root: download the staged BASE tree (`bench.ref`, `deps-base.tgz`; the candidate is never staged for this job), verify
 *      the digest, unpack it;
 *   2. root: verify the unprivileged user exists and cannot reach instance metadata (same checks as the bench, exit 12-14);
 *   3. as the unprivileged `ratchet-deps` user, `--no-new-privs`, `env -i` plus the proxy variables only (no AWS_*): the strategy's
 *      fetch step (`pnpm fetch` / `npm ci --ignore-scripts`). It fills the store; it installs no node_modules and runs no
 *      lifecycle script of any package;
 *   4. root: prune per-project state, refuse symlinks, pack the store (zstd if the image has it, else gzip), digest it;
 *   5. root: upload the archive to `bench-deps/<key>/<sha256>.<ext>` and THEN the manifest `bench-deps/<key>.json`.
 *
 * The user that touches the network and downloaded packages never holds a credential, and the user that holds the credential
 * (root, via the instance profile) never executes anything from the network.
 */

/** Exit codes worth recognising (the executor reports the stderr tail with them). */
export const DEPS_BUILD_EXIT = { prepare: 10, download: 20, digest: 21, unpack: 22, fetch: 50, symlink: 51, pack: 52, uploadArchive: 53, uploadManifest: 54 } as const;

export interface DepsBuildInput {
  layout: Layout;
  strategy: DepsStrategy;
  /** The staged BASE tree (`deps-base.tgz`). */
  base: StagedArchive;
  key: string;
  /** Hashes of the base tree's lockfiles (as the controller computed them). */
  lockfiles: Readonly<Record<string, string>>;
  image: string;
  arch: string;
  /** Artifacts bucket the archive and the manifest are uploaded to. */
  bucket: string;
  /** Limit for the fetch step. */
  timeoutMs: number;
  /** `none` runs everything as the current user (tests only). */
  isolation: Isolation;
  /**
   * The repo's container image (`environment.image`): the fetch runs INSIDE it (the bench instances have no node or package
   * manager of their own), as the same unprivileged user, with all capabilities dropped and a read-only root, but WITH network
   * (this instance is the one with egress). The image is loaded from the artifacts bucket when this instance does not hold it yet.
   */
  container?: ImageRestore;
}

const BUCKET_RE = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/;
const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

export function depsBuildScript(o: DepsBuildInput): string {
  if (o.strategy.id === 'none') throw new BenchError('there is nothing to snapshot for a strategy without a lockfile');
  if (!/^(pnpm|npm)-[0-9a-f]{40}$/.test(o.key)) throw new BenchError(`snapshot key ${JSON.stringify(o.key)} is not a plain key`);
  if (!BUCKET_RE.test(o.bucket)) throw new BenchError(`artifacts bucket ${JSON.stringify(o.bucket)} is not a valid bucket name`);
  if (o.container) assertContainerRuntime(o.container.rt, o.layout.work);
  assertSafeRef(o.base.commit, 'base commit');
  if (!/^s3:\/\/[a-z0-9][a-z0-9.-]*\/[A-Za-z0-9._\/-]+\/deps-base\.tgz$/.test(o.base.uri) || o.base.uri.includes('..')) {
    throw new BenchError(`staged base tree uri ${JSON.stringify(o.base.uri)} is not a plain s3:// uri ending in deps-base.tgz`);
  }
  if (!/^[0-9a-f]{64}$/.test(o.base.sha256)) throw new BenchError('staged base tree digest is not a sha256 hex string');
  const isolated = o.isolation === 'user';
  const store = `${o.layout.work}/store`;
  const fetchEnv = Object.entries(o.strategy.fetchEnv(store));
  for (const [k] of fetchEnv) if (!ENV_NAME.test(k)) throw new BenchError(`invalid environment variable name ${k}`);
  const envArgs = fetchEnv.map(([k, v]) => `${k}=${sq(v)}`).join(' ');
  const timeoutS = Math.max(1, Math.ceil(o.timeoutMs / 1000));
  const head = JSON.stringify({ v: 1, key: o.key, strategy: o.strategy.id, rev: o.strategy.rev, lockfiles: o.lockfiles, image: o.image, arch: o.arch });
  const archivePrefix = `${DEPS_PREFIX}${o.key}`;

  // The fetch command, as the unprivileged user: a clean environment plus only the proxy variables the user data wrote.
  const clean = `env -i PATH="$PATH" HOME="$W/home" TMPDIR="$W/tmp" LANG=C.UTF-8 USER="$DU" LOGNAME="$DU" \${PX[@]+"\${PX[@]}"} ${envArgs} bash -c ${sq(o.strategy.fetch)}`;
  const run = isolated ? `setpriv --reuid="$DU" --regid="$DU" --clear-groups --no-new-privs -- ${clean}` : clean;
  const fetchCommand = o.container
    ? containerRun({
        rt: o.container.rt,
        name: '"$CN"',
        user: '"$BUID:$BGID"',
        noNewPrivileges: true,
        network: 'bridge',
        workdir: '"$TREE"',
        mounts: ['$TREE:$TREE', '$STORE:$STORE', '$W/home:$W/home', '$W/tmp:$W/tmp'],
        env: ['HOME=$W/home', 'TMPDIR=$W/tmp'],
        extra: [...fetchEnv.flatMap(([k, v]) => ['-e', sq(`${k}=${v}`)]), '${DENV[@]+"${DENV[@]}"}'],
        command: sq(o.strategy.fetch),
      })
    : run;
  const tool = (cmd: string): string => (o.container ? `ctr ${sq(cmd)}` : cmd);

  return `set -uo pipefail
W=${sq(o.layout.work)}
TREE="$W/tree"; STORE=${sq(store)}; OUT="$W/out"
emit() { printf '%s %s %s\\n' '${SENTINEL}' "$1" "$2"; }
rm -rf -- "\${W:?}"; mkdir -p "$TREE" "$STORE" "$OUT" "$W/in" "$W/home" "$W/tmp" || exit ${DEPS_BUILD_EXIT.prepare}
chmod 755 "$W"
aws s3 cp --only-show-errors ${sq(o.base.uri)} "$W/in/base.tgz" || { echo "cannot download the base tree ${o.base.uri}" >&2; exit ${DEPS_BUILD_EXIT.download}; }
[ "$(sha256sum -- "$W/in/base.tgz" | cut -d' ' -f1)" = ${sq(o.base.sha256)} ] || { echo "sha256 mismatch for the base tree" >&2; exit ${DEPS_BUILD_EXIT.digest}; }
tar -xzf "$W/in/base.tgz" -C "$TREE" --no-same-owner || exit ${DEPS_BUILD_EXIT.unpack}
rm -f "$W/in/base.tgz"
DU=${sq(isolated ? DEPS_USER : 'deps')}
${
  isolated
    ? `id -u "$DU" >/dev/null 2>&1 || { echo "snapshot user $DU does not exist on this image" >&2; exit 12; }
iptables -C OUTPUT -m owner --uid-owner "$DU" -d 169.254.169.254 -j REJECT >/dev/null 2>&1 || { echo "IMDS is not blocked for $DU (iptables owner rule missing)" >&2; exit 13; }
if setpriv --reuid="$DU" --regid="$DU" --clear-groups --no-new-privs -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo "instance metadata is reachable as $DU" >&2; exit 14; fi
chown -R "$DU:" "$TREE" "$STORE" "$W/home" "$W/tmp" || exit 16`
    : ''
}
[ -r /etc/ratchet/runner.env ] && { set -a; . /etc/ratchet/runner.env; set +a; }
PX=(); for v in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy NO_PROXY no_proxy NODE_USE_ENV_PROXY; do [ -n "\${!v:-}" ] && PX+=("$v=\${!v}"); done
${
  o.container
    ? `DENV=(); for kv in \${PX[@]+"\${PX[@]}"}; do DENV+=(-e "$kv"); done
${isolated ? 'BUID=$(id -u "$DU"); BGID=$(id -g "$DU")' : 'BUID=$(id -u); BGID=$(id -g)'}
CN="ratchet-$(basename -- "$W")-fetch"
${ensureImageLoaded(o.container)}${ctrFunction(o.container.rt)}docker rm -f "$CN" >/dev/null 2>&1 || true
`
    : ''
}cd "$TREE" || exit ${DEPS_BUILD_EXIT.prepare}
s=$(date +%s%N)
timeout -k 10 ${timeoutS} ${fetchCommand} </dev/null >"$W/fetch.out" 2>&1; rc=$?
${o.container ? 'docker rm -f "$CN" >/dev/null 2>&1 || true\n' : ''}e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
echo "--- fetch output tail ---"; tail -c 4000 "$W/fetch.out"
[ $rc -eq 0 ] || { echo "the package fetch (${o.strategy.id}) exited with status $rc" >&2; exit ${DEPS_BUILD_EXIT.fetch}; }

${o.strategy.prune}
if [ -n "$(find "$STORE" -type l -print -quit)" ]; then echo "the fetched store contains symlinks; refusing to pack it" >&2; exit ${DEPS_BUILD_EXIT.symlink}; fi
if command -v zstd >/dev/null 2>&1; then
  EXT=tar.zst; COMP=zstd
  tar -C "$STORE" --sort=name --owner=0 --group=0 --numeric-owner -cf - . | zstd -T0 -3 -q -o "$OUT/snapshot" || exit ${DEPS_BUILD_EXIT.pack}
else
  EXT=tgz; COMP=gzip
  tar -C "$STORE" --sort=name --owner=0 --group=0 --numeric-owner -cf - . | gzip -3 > "$OUT/snapshot" || exit ${DEPS_BUILD_EXIT.pack}
fi
SHA=$(sha256sum -- "$OUT/snapshot" | cut -d' ' -f1); BYTES=$(stat -c %s -- "$OUT/snapshot")
NODEV=$(${tool('node --version')} 2>/dev/null | tr -cd 'A-Za-z0-9._+-'); PMV=$(${tool(o.strategy.version)} 2>/dev/null | tr -cd 'A-Za-z0-9._+-')
[ -n "$NODEV" ] && [ -n "$PMV" ] || { echo "cannot determine the node and ${o.strategy.id} versions" >&2; exit ${DEPS_BUILD_EXIT.pack}; }
ARCHIVE=${sq(archivePrefix)}/"$SHA.$EXT"
printf '%s,"node":"%s","pm":"%s","compression":"%s","archive":"%s","sha256":"%s","bytes":%s,"createdAt":%s}\\n' ${sq(head.slice(0, -1))} "$NODEV" "$PMV" "$COMP" "$ARCHIVE" "$SHA" "$BYTES" "$(date +%s%3N)" > "$OUT/manifest.json"
aws s3 cp --only-show-errors "$OUT/snapshot" "s3://${o.bucket}/$ARCHIVE" || { echo "cannot upload the snapshot" >&2; exit ${DEPS_BUILD_EXIT.uploadArchive}; }
aws s3 cp --only-show-errors "$OUT/manifest.json" ${sq(`s3://${o.bucket}/${archivePrefix}.json`)} || { echo "cannot upload the manifest" >&2; exit ${DEPS_BUILD_EXIT.uploadManifest}; }
emit deps "{\\"key\\":\\"${o.key}\\",\\"sha256\\":\\"$SHA\\",\\"bytes\\":$BYTES,\\"compression\\":\\"$COMP\\",\\"ms\\":$ms}"
exit 0
`;
}
