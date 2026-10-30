import { extname } from 'node:path';
import { globToRegExp } from '@ratchet/core';
import type { DepsStrategy } from './deps.js';
import { BenchError } from './errors.js';
import { assertSafeRef, BENCH_USER, HOLDOUT_USER, sq, SENTINEL } from './shell.js';
import {
  containerRun,
  assertContainerRuntime,
  helperMounts,
  HELPER_IN_CONTAINER,
  idsFragment,
  imageRestoreFragment,
  launcherConfFragment,
  type ContainerRuntime,
  type ImageRestore,
} from './image-scripts.js';

/**
 * Shell scripts for the remote (bench instance) pipeline. One script per step; every script is
 * wrapped by `wrapStep` so it runs under bash even when the SSM document uses `sh`.
 *
 * Results travel on stdout as sentinel lines `@@RATCHET@@ <kind> <json>`, because SSM returns at
 * most ~24,000 characters of output per command: the bench JSON is sent as the last 12,000 bytes of
 * stdout, base64 encoded, and each bench run is its own command so the budget is per run.
 */

// SENTINEL, BENCH_USER, HOLDOUT_USER, sq and assertSafeRef live in shell.ts (shared with the other script builders).
export { assertSafeRef, BENCH_USER, HOLDOUT_USER, sq, SENTINEL };
/**
 * Root-installed launcher (packages/aws `benchUserData`): the harness runs `$RATCHET_CANDIDATE_EXEC <cmd...>`
 * to start a candidate process as BENCH_USER, in the candidate's checkout, with a clean environment (no
 * `RATCHET_TRACE`, no `AWS_*`). stdin/stdout/stderr are the caller's: give the candidate pipes or `ignore`, not
 * the harness's own stdout, which carries the result.
 */
export const CANDIDATE_LAUNCHER = '/usr/local/bin/ratchet-candidate';

/** Where one archive was staged (see stage.ts) and the digest the instance must see. */
export interface StagedArchive {
  /** `s3://<artifacts bucket>/bench-inputs/<opt>/<job>/<name>.tgz` */
  uri: string;
  sha256: string;
  /** Full commit sha the archive was made from. */
  commit: string;
  /** sha256 of the requested root-level files of the staged tree (lockfiles), computed on the controller before archiving. */
  files?: Record<string, string>;
  /** sha256 per file of the repo's container environment in the staged tree (see image.ts `environmentFilesOf`), when the job runs in a container. */
  env?: Record<string, string>;
}

/** A dependency snapshot to restore before setup (see deps.ts). */
export interface DepsRestore {
  strategy: DepsStrategy;
  /** `s3://<artifacts bucket>/bench-deps/<key>/<sha256>.<ext>` */
  uri: string;
  sha256: string;
  compression: 'zstd' | 'gzip';
  /** `node --version` and the package manager's version on the machine that built it; a different image is refused. */
  node: string;
  pm: string;
}

/** Where the restored store lives on the instance (read-only) and the directory the candidate user may write for the package manager. */
export const depsPaths = (l: Layout): { store: string; scratch: string } => ({ store: `${l.work}/deps/store`, scratch: `${l.work}/home/.ratchet-deps` });

/** `user`: setup and guardrails run as BENCH_USER, the bench harness as HOLDOUT_USER, IMDS blocked for both (default, fail-closed). `none`: single-user, for local tests only. */
export type Isolation = 'user' | 'none';
const HEREDOC = '__RATCHET_STEP_EOF__';

/** Make a bash script safe to hand to a shell that may not be bash: write it to a file and run it with bash. */
export function wrapStep(body: string): string {
  if (body.includes(HEREDOC)) throw new BenchError('script contains the step delimiter');
  return `f=$(mktemp)\ncat > "$f" <<'${HEREDOC}'\n${body}\n${HEREDOC}\nbash "$f"\nrc=$?\nrm -f "$f"\nexit $rc\n`;
}

function normalizeGlob(g: string): string {
  return g.replace(/\\/g, '/').replace(/^(\.\/)+/, '').replace(/^\/+/, '').replace(/\/+$/, '');
}

/**
 * PCRE (for `grep -zP`) equivalent of core's `matchesGlob` for a list of globs: a path matches when
 * some leading directory prefix (or the whole path) matches one of the globs. The glob -> regex
 * translation is core's own (`globToRegExp`), so local and remote runners agree on what is protected.
 */
export function protectedPcre(globs: readonly string[]): string {
  const bodies = globs
    .map(normalizeGlob)
    .filter((g) => g !== '')
    .map((g) => globToRegExp(g).source.slice(1, -1));
  if (bodies.length === 0) return '(?!)';
  return `(?s)^(?:${bodies.join('|')})(?:/.*)?\\z`;
}

export interface Layout {
  /** Per-job working directory on the instance. */
  work: string;
}

const prelude = (l: Layout) => `set -uo pipefail
W=${sq(l.work)}
CODE="$W/code"; STAGE="$W/stage"; G="$W/repo.git"; H="$W/harness"
emit() { printf '%s %s %s\\n' '${SENTINEL}' "$1" "$2"; }
b64() { base64 | tr -d '\\n'; }
`;

/** Functions that implement the protected overlay (see overlay.ts for the rule). */
const overlayFns = `
parents_real() {
  local d acc="$CODE" part; local -a parts
  d=$(dirname -- "$1"); [ "$d" = . ] && return 0
  IFS=/ read -ra parts <<< "$d"
  for part in "\${parts[@]}"; do acc="$acc/$part"; [ -L "$acc" ] && return 1; [ -d "$acc" ] || return 1; done
  return 0
}
ensure_parents() {
  local d acc="$CODE" part; local -a parts
  d=$(dirname -- "$1"); [ "$d" = . ] && return 0
  IFS=/ read -ra parts <<< "$d"
  for part in "\${parts[@]}"; do
    acc="$acc/$part"
    if [ -L "$acc" ] || { [ -e "$acc" ] && [ ! -d "$acc" ]; }; then rm -f -- "\${acc:?}"; fi
    [ -d "$acc" ] || mkdir -- "$acc"
  done
}
same() {
  if [ -L "$1" ]; then [ -L "$2" ] && [ "$(readlink -- "$1")" = "$(readlink -- "$2")" ]
  else [ -f "$2" ] && [ ! -L "$2" ] && cmp -s -- "$1" "$2"; fi
}
copied=0
apply() {
  local mode="$1" p
  : > "$W/changed.list"
  while IFS= read -r -d '' p; do
    if [ "$mode" = check ] && parents_real "$p" && same "$STAGE/$p" "$CODE/$p"; then continue; fi
    [ "$mode" = check ] && printf '%s\\0' "$p" >> "$W/changed.list"
    ensure_parents "$p"
    rm -rf -- "\${CODE:?}/\${p:?}"
    cp -P -p -- "$STAGE/$p" "$CODE/$p" || return 1
    copied=$((copied+1))
  done < "$W/bench.match"
}
`;

/** Wait-for-instance probe. */
export const pingScript = (): string => 'echo ready\n';

/**
 * Environment the candidate launcher reads (it runs as BENCH_USER through sudo, which keeps exactly these
 * variables). Set by root before any harness starts.
 */
const candidateEnv = `export RATCHET_CANDIDATE_DIR="$CODE" RATCHET_CANDIDATE_HOME="$W/home" RATCHET_CANDIDATE_TMP="$W/tmp" RATCHET_CANDIDATE_PATH="$PATH"
`;

/**
 * Checks that the instance really has the two unprivileged users, the IMDS block for both, the launcher, and
 * that the holdout directory is closed to the candidate's user and open to the harness's; refuses to continue
 * otherwise (exit 12 to 19). Then hands the code tree to the candidate user and the scratch dirs to the harness user.
 */
const isolationCheck = (isolation: Isolation, launcher: string, container: boolean): string =>
  isolation === 'none'
    ? ''
    : `BU=${sq(BENCH_USER)}; HU=${sq(HOLDOUT_USER)}; LAUNCHER=${sq(launcher)}
id -u "$BU" >/dev/null 2>&1 || { echo "bench user $BU does not exist on this image" >&2; exit 12; }
id -u "$HU" >/dev/null 2>&1 || { echo "holdout user $HU does not exist on this image" >&2; exit 12; }
for u in "$BU" "$HU"; do
  iptables -C OUTPUT -m owner --uid-owner "$u" -d 169.254.169.254 -j REJECT >/dev/null 2>&1 || { echo "IMDS is not blocked for $u (iptables owner rule missing)" >&2; exit 13; }
done
if setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo "instance metadata is reachable as $BU" >&2; exit 14; fi
if setpriv --reuid="$HU" --regid="$HU" --clear-groups -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo "instance metadata is reachable as $HU" >&2; exit 14; fi
mkdir -p "$W/home" "$W/tmp" "$W/hhome" "$W/htmp" "$W/out" "$W/holdout" || exit 15
chown -R "$BU:" "$CODE" "$W/home" "$W/tmp" || exit 16
chown -R "$HU:" "$W/hhome" "$W/htmp" "$W/out" "$W/holdout" || exit 16
chmod 700 "$W/holdout"
chmod 755 "$W"
${candidateEnv}printf x > "$W/holdout/.probe" && chown "$HU:" "$W/holdout/.probe" && chmod 600 "$W/holdout/.probe" || exit 15
if setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- cat "$W/holdout/.probe" >/dev/null 2>&1; then echo "the holdout directory is readable by $BU" >&2; exit 17; fi
setpriv --reuid="$HU" --regid="$HU" --clear-groups -- cat "$W/holdout/.probe" >/dev/null 2>&1 || { echo "the holdout directory is not readable by $HU" >&2; exit 19; }
rm -f "$W/holdout/.probe"
${
  container
    ? '' // the harness starts candidate processes through the container launcher, which the image restore probes
    : `[ "$(setpriv --reuid="$HU" --regid="$HU" --clear-groups -- "$LAUNCHER" id -un 2>/dev/null)" = "$BU" ] || { echo "the candidate launcher $LAUNCHER does not run commands as $BU for $HU" >&2; exit 18; }
`
}`;

const VERSION_RE = /^[A-Za-z0-9._+-]{1,64}$/;

/**
 * Restores the dependency snapshot as ROOT, after the isolation checks and before any step runs candidate code:
 * download (through the S3 gateway endpoint), sha256 check, same node and package-manager version as the builder, extract,
 * refuse symlinks, let the strategy wire its few writable paths, then hand the whole store to root and make it read-only.
 * The candidate user can read the store (setup installs from it) but not change it; the only things it owns are
 * `$SCRATCH` (the package manager's per-project bookkeeping, outside the store). Exit 25 to 29.
 */
function depsRestoreFragment(r: DepsRestore, isolated: boolean, container: boolean): string {
  if (!/^s3:\/\/[a-z0-9][a-z0-9.-]*\/bench-deps\/[A-Za-z0-9._\/-]+$/.test(r.uri) || r.uri.includes('..')) throw new BenchError(`snapshot uri ${JSON.stringify(r.uri)} is not a plain s3:// uri under bench-deps/`);
  if (!/^[0-9a-f]{64}$/.test(r.sha256)) throw new BenchError('snapshot digest is not a sha256 hex string');
  if (!VERSION_RE.test(r.node) || !VERSION_RE.test(r.pm)) throw new BenchError('snapshot tool versions are not plain version strings');
  // In container mode the toolchain is the image's: the versions are asked from a throwaway container (offline, unprivileged).
  const tool = (cmd: string): string => (container ? `ctr ${sq(cmd)}` : cmd);
  const extract =
    r.compression === 'zstd'
      ? `command -v zstd >/dev/null 2>&1 || { echo "zstd is missing on this image (rebuild the bench image)" >&2; exit 28; }
zstd -dc -- "$D/in/snapshot" | tar -x -C "$STORE" --no-same-owner --no-same-permissions || exit 29`
      : `tar -xzf "$D/in/snapshot" -C "$STORE" --no-same-owner --no-same-permissions || exit 29`;
  return `# ---- dependency snapshot (root): read-only for the candidate user ----
D="$W/deps"; STORE="$D/store"; SCRATCH="$W/home/.ratchet-deps"
mkdir -p "$STORE" "$D/in" "$SCRATCH" || exit 25
aws s3 cp --only-show-errors ${sq(r.uri)} "$D/in/snapshot" || { echo "cannot download the dependency snapshot ${r.uri}" >&2; exit 25; }
[ "$(sha256sum -- "$D/in/snapshot" | cut -d' ' -f1)" = ${sq(r.sha256)} ] || { echo "sha256 mismatch for the dependency snapshot" >&2; exit 26; }
[ "$(${tool('node --version')} 2>/dev/null)" = ${sq(r.node)} ] || { echo "the dependency snapshot was built with node ${r.node}, this ${container ? 'container' : 'instance'} has $(${tool('node --version')} 2>&1)" >&2; exit 27; }
[ "$(${tool(r.strategy.version)} 2>/dev/null)" = ${sq(r.pm)} ] || { echo "the dependency snapshot was built with ${r.strategy.id} ${r.pm}, this ${container ? 'container' : 'instance'} has $(${tool(r.strategy.version)} 2>&1)" >&2; exit 27; }
${extract}
rm -f "$D/in/snapshot"
if [ -n "$(find "$STORE" -type l -print -quit)" ]; then echo "the dependency snapshot contains symlinks" >&2; exit 29; fi
${r.strategy.restore}
${isolated ? 'chown -R "$BU:" "$SCRATCH" || exit 16\nchown -hR root:root "$D" || exit 16\n' : ''}chmod -R go-w,a+rX "$D" || exit 16
emit deps "{\\"restored\\":true,\\"bytes\\":$(du -sb "$STORE" | cut -f1)}"
`;
}

/**
 * Fetches the pre-staged candidate and bench.ref archives from the artifacts bucket (through the S3
 * gateway endpoint; the bench subnet has no other way out), verifies their digests, applies the
 * protected overlay, and - with `isolation: 'user'` - verifies the two unprivileged users, the IMDS
 * block, the holdout directory permissions and the candidate launcher, and hands the code tree to the
 * candidate user. No source-control access and no credentials in the script, and the sealed bucket is
 * not touched: the holdout is fetched later, by the bench step that needs it.
 *
 * With `deps` the dependency snapshot is restored (read-only) last, still as root and before any candidate code ran.
 * `pinned` are extra protected paths (the lockfiles): they come from bench.ref exactly like `globs`, but do not count
 * as "the user protected something" in the `matched` figure.
 */
export function checkoutScript(o: {
  layout: Layout;
  code: StagedArchive;
  bench: StagedArchive;
  globs: readonly string[];
  /** Always-protected root paths (lockfiles of the dependency strategy), on top of `globs`. */
  pinned?: readonly string[];
  prune: boolean;
  isolation: Isolation;
  /** Restore this dependency snapshot before setup. */
  deps?: DepsRestore;
  /** Load the repo's container image (offline) and probe it before setup; setup, guardrails and the evaluator then run in it. */
  image?: ImageRestore;
  /** Test seam; default `CANDIDATE_LAUNCHER`. */
  candidateLauncher?: string;
}): string {
  for (const a of [o.code, o.bench]) {
    assertSafeRef(a.commit, 'staged commit');
    if (!/^s3:\/\/[a-z0-9][a-z0-9.-]*\/[A-Za-z0-9._\/-]+$/.test(a.uri) || a.uri.includes('..')) throw new BenchError(`staged archive uri ${JSON.stringify(a.uri)} is not a plain s3:// uri`);
    if (!/^[0-9a-f]{64}$/.test(a.sha256)) throw new BenchError('staged archive digest is not a sha256 hex string');
  }
  return `${prelude(o.layout)}${overlayFns}
RE=${sq(protectedPcre([...o.globs, ...(o.pinned ?? [])]))}; RE_USER=${sq(protectedPcre(o.globs))}
rm -rf -- "\${W:?}"; mkdir -p "$CODE" "$STAGE" "$W/logs" "$W/in" || exit 10
fetch() {
  aws s3 cp --only-show-errors "$1" "$3" || { echo "cannot download $1" >&2; return 1; }
  [ "$(sha256sum -- "$3" | cut -d' ' -f1)" = "$2" ] || { echo "sha256 mismatch for $1" >&2; return 2; }
}
fetch ${sq(o.code.uri)} ${sq(o.code.sha256)} "$W/in/code.tgz" || exit 20
fetch ${sq(o.bench.uri)} ${sq(o.bench.sha256)} "$W/in/bench.tgz" || exit 21
tar -xzf "$W/in/code.tgz" -C "$CODE" --no-same-owner || exit 22
tar -xzf "$W/in/bench.tgz" -C "$STAGE" --no-same-owner || exit 23
list() { (cd -- "$1" && find . -mindepth 1 \\( -type f -o -type l \\) -printf '%P\\0'); }
list "$STAGE" | grep -zP "$RE" > "$W/bench.match" || true
matched=$(list "$STAGE" | grep -zcP "$RE_USER" || true)
list "$CODE" | grep -zP "$RE" > "$W/code.match" || true
removed=0
if [ ${o.prune ? 1 : 0} = 1 ]; then
  CODE_REAL=$(cd "$CODE" && pwd -P)
  while IFS= read -r -d '' p; do
    d=$(dirname -- "$p")
    if [ "$d" = . ] || [ "$(realpath -m -- "$CODE/$d")" = "$CODE_REAL/$d" ]; then
      rm -rf -- "\${CODE:?}/\${p:?}"
      grep -zqxF -- "$p" "$W/bench.match" || removed=$((removed+1))
    fi
  done < "$W/code.match"
fi
apply init || exit 24
${isolationCheck(o.isolation, o.candidateLauncher ?? CANDIDATE_LAUNCHER, !!o.image)}${o.image ? imageRestoreFragment(o.image, { work: o.layout.work, isolated: o.isolation === 'user' }) : ''}${o.deps ? depsRestoreFragment(o.deps, o.isolation === 'user', !!o.image) : ''}emit checkout "{\\"code\\":\\"${o.code.commit}\\",\\"bench\\":\\"${o.bench.commit}\\",\\"removed\\":$removed,\\"copied\\":$copied,\\"matched\\":$matched,\\"isolation\\":\\"${o.isolation}\\",\\"container\\":${o.image ? 'true' : 'false'}}"
`;
}

export interface StepScriptInput {
  layout: Layout;
  kind: 'setup' | 'guardrail' | 'bench';
  /** Used for log file names; sanitized. */
  name: string;
  command: string;
  timeoutMs: number;
  env: Record<string, string>;
  /** Default 'user'. */
  isolation?: Isolation;
  /**
   * Confirm bench runs only: the sealed holdout to fetch for THIS run (`s3://...`, or a path / `file://` in tests).
   * The step fetches it as root, hands it to the harness user alone (mode 0600), exports `RATCHET_TRACE` to the
   * harness and wipes it when the run is over. Never set for setup, guardrails, baseline or bench-phase runs: those
   * scripts do not contain the sealed location at all.
   */
  holdout?: { uri: string };
  /**
   * Run the step INSIDE the repo's container image (`environment.image`), loaded at checkout: `docker run --network none`, the
   * same numeric uids as the host path, all capabilities dropped (the harness keeps SETUID/SETGID for its launcher), read-only
   * root, tmpfs /tmp, and only the checkout, the homes, the out dir (and, for a confirm harness, the holdout, read-only) mounted.
   */
  container?: ContainerRuntime;
  /** The dependency snapshot was restored at `$W/deps`: mount it read-only into setup / guardrail containers. */
  depsMounted?: boolean;
  /** Test seam; default `CANDIDATE_LAUNCHER`. */
  candidateLauncher?: string;
}

/** Exit codes of the holdout fetch inside a bench step (the executor turns them into an infrastructure error). */
export const HOLDOUT_FETCH_EXIT = { prepare: 40, download: 41, permissions: 42 } as const;

/**
 * Setup, guardrail and bench steps. Before running the command the script verifies that the
 * protected files still equal bench.ref's; if not it restores them, reports `tampered`, and exits
 * 87 without running the command.
 *
 * With `isolation: 'user'` there are two unprivileged principals:
 *
 *  - setup and guardrails run as BENCH_USER (candidate code, `--no-new-privs`);
 *  - a BENCH step (the evaluator harness) runs as HOLDOUT_USER, from a root-owned read-only SNAPSHOT of the
 *    built tree taken right before the first run, so the candidate cannot edit what the harness executes.
 *    The harness starts candidate processes through `$RATCHET_CANDIDATE_EXEC`, which runs them as BENCH_USER.
 *    The holdout (confirm only) is fetched here, after setup and guardrails have finished, written mode 0600
 *    for HOLDOUT_USER and wiped after the run (also on any exit), so no candidate process ever has a window
 *    in which it could open it.
 *
 * The harness can run unprivileged-but-not-`no-new-privs`: the launcher is a sudo rule, which setuid
 * needs. Everything the harness executes should therefore come from `protected` paths.
 */
export function stepScript(o: StepScriptInput): string {
  const timeoutS = Math.max(1, Math.ceil(o.timeoutMs / 1000));
  const file = o.name.replace(/[^A-Za-z0-9._-]/g, '_').slice(0, 60);
  for (const k of Object.keys(o.env)) if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(k)) throw new BenchError(`invalid environment variable name ${k}`);
  if (o.holdout && o.kind !== 'bench') throw new BenchError('the holdout is only ever given to bench steps');
  if (o.env.RATCHET_TRACE !== undefined) throw new BenchError('RATCHET_TRACE is set by the step script itself, from `holdout`');
  const exports = Object.entries(o.env)
    .map(([k, v]) => `export ${k}=${sq(v)}`)
    .join('\n');
  const isolated = (o.isolation ?? 'user') === 'user';
  const harness = isolated && o.kind === 'bench';
  const launcher = o.candidateLauncher ?? CANDIDATE_LAUNCHER;
  const result =
    o.kind === 'bench'
      ? `emit run "{\\"exit\\":$rc,\\"ms\\":$ms,\\"timedOut\\":$to,\\"stdout_b64\\":\\"$(tail -c 12000 "$OUT.stdout" | b64)\\"}"
${
  o.holdout
    ? // The log of a holdout run carries no free text from the run: not the harness's stderr (which can relay
      // whatever the candidate's processes wrote to it).
      'echo "--- stderr withheld: holdout run ---"'
    : 'echo "--- stderr tail ---"; tail -c 3000 "$OUT.stderr"'
}`
      : `emit step "{\\"exit\\":$rc,\\"ms\\":$ms,\\"timedOut\\":$to}"
echo "--- output tail ---"; { tail -c 4000 "$OUT.stdout"; tail -c 2000 "$OUT.stderr"; }`;

  // Setup and guardrails: candidate code as BENCH_USER without new privileges. The bench harness: HOLDOUT_USER,
  // with its own HOME/TMPDIR/output dir. Both lose the AWS_* variables even though a stock SSM environment has none.
  const scrub = 'env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN -u AWS_PROFILE';
  const container = o.container;
  if (container) assertContainerRuntime(container, o.layout.work);
  const passEnv = [...Object.keys(o.env), ...(o.holdout ? ['RATCHET_TRACE'] : [])];
  const containerCmd = (): string => {
    if (!container) return '';
    if (harness) {
      return containerRun({
        rt: container,
        name: '"$CN"',
        user: '"$HUID:$HGID"',
        capAdd: ['SETUID', 'SETGID'], // only for the launcher; the harness itself needs nothing
        noNewPrivileges: false, // the launcher is setuid root; it sets no_new_privs on every candidate process
        network: 'none',
        workdir: '"$H"',
        mounts: [
          '$H:$H:ro',
          '$CODE:$CODE',
          '$W/home:$W/home',
          '$W/tmp:$W/tmp',
          '$W/hhome:$W/hhome',
          '$W/htmp:$W/htmp',
          '$W/out:$W/out',
          ...(o.holdout ? ['$W/holdout:$W/holdout:ro'] : []),
          ...helperMounts(container),
        ],
        env: [
          'HOME=$W/hhome',
          'TMPDIR=$W/htmp',
          'USER=$HU',
          'LOGNAME=$HU',
          'RATCHET_OUT_DIR=$W/out',
          `RATCHET_CANDIDATE_EXEC=${HELPER_IN_CONTAINER}`,
          'RATCHET_CANDIDATE_DIR=$CODE',
          'RATCHET_CANDIDATE_HOME=$W/home',
          'RATCHET_CANDIDATE_TMP=$W/tmp',
        ],
        passEnv,
        command: sq(o.command),
      });
    }
    return containerRun({
      rt: container,
      name: '"$CN"',
      user: '"$BUID:$BGID"',
      noNewPrivileges: true,
      network: 'none',
      workdir: '"$CODE"',
      mounts: ['$CODE:$CODE', '$W/home:$W/home', '$W/tmp:$W/tmp', ...(o.depsMounted && o.kind !== 'bench' ? ['$W/deps:$W/deps:ro'] : []), ...(o.holdout ? ['$W/holdout:$W/holdout:ro'] : [])],
      env: ['HOME=$W/home', 'TMPDIR=$W/tmp', ...(isolated ? ['USER=$BU', 'LOGNAME=$BU'] : [])],
      passEnv,
      command: sq(o.command),
    });
  };
  const run = container
    ? containerCmd()
    : harness
    ? `setpriv --reuid="$HU" --regid="$HU" --clear-groups -- ${scrub} HOME="$W/hhome" TMPDIR="$W/htmp" USER="$HU" LOGNAME="$HU" RATCHET_OUT_DIR="$W/out" RATCHET_CANDIDATE_EXEC=${sq(launcher)} bash -c ${sq(o.command)}`
    : isolated
      ? `setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- ${scrub} HOME="$W/home" TMPDIR="$W/tmp" USER="$BU" LOGNAME="$BU" bash -c ${sq(o.command)}`
      : `bash -c ${sq(o.command)}`;

  const holdoutPath = o.holdout ? localTracePath(o.layout, o.holdout.uri) : undefined;
  const fetchHoldout = o.holdout
    ? `
# ---- holdout: fetched only now (after setup and guardrails), by root, for this run alone ----
HD="$W/holdout"; TRACE=${sq(holdoutPath ?? '')}; URI=${sq(o.holdout.uri)}
wipe() { shred -u -- "$TRACE" 2>/dev/null || rm -f -- "$TRACE"; rm -rf -- "\${HD:?}"/* 2>/dev/null || true; }
trap wipe EXIT
mkdir -p "$HD" || exit ${HOLDOUT_FETCH_EXIT.prepare}
case "$URI" in
  s3://*) aws s3 cp --only-show-errors "$URI" "$TRACE" || exit ${HOLDOUT_FETCH_EXIT.download} ;;
  file://*) cp -- "\${URI#file://}" "$TRACE" || exit ${HOLDOUT_FETCH_EXIT.download} ;;
  *) cp -- "$URI" "$TRACE" || exit ${HOLDOUT_FETCH_EXIT.download} ;;
esac
${isolated ? 'chown "$HU:" "$TRACE" || exit ' + HOLDOUT_FETCH_EXIT.permissions + '\nchmod 600 "$TRACE" || exit ' + HOLDOUT_FETCH_EXIT.permissions : 'chmod 600 "$TRACE" || exit ' + HOLDOUT_FETCH_EXIT.permissions}
emit holdout "{\\"bytes\\":$(wc -c < "$TRACE" | tr -d ' ')}"
export RATCHET_TRACE="$TRACE"
`
    : '';

  const snapshot = harness
    ? `
# ---- read-only snapshot of the built tree: what the harness (HOLDOUT_USER) executes ----
snapshot() {
  rm -rf -- "$W/harness.tmp"
  cp -a --reflink=auto -- "$CODE" "$W/harness.tmp" || return 1
  find "$W/harness.tmp" -type f -perm /6000 -exec chmod ug-s {} + || return 1
  chown -hR root:root "$W/harness.tmp" || return 1
  chmod -R go-w,a+rX "$W/harness.tmp" || return 1
  mv -- "$W/harness.tmp" "$H"
}
[ -d "$H" ] || snapshot || { echo "cannot snapshot the built tree for the evaluator" >&2; exit 31; }
`
    : '';

  return `${prelude(o.layout)}${overlayFns}
${
  isolated
    ? `BU=${sq(BENCH_USER)}; HU=${sq(HOLDOUT_USER)}
kill_users() { pkill -KILL -u "$BU" 2>/dev/null || true; pkill -KILL -u "$HU" 2>/dev/null || true; }
kill_users
${candidateEnv}`
    : ''
}${
  container
    ? `${idsFragment(isolated)}CN="ratchet-$(basename -- "$W")-${file.replace(/_/g, '-')}"
docker rm -f "$CN" >/dev/null 2>&1 || true
`
    : ''
}
apply check
if [ -s "$W/changed.list" ]; then
  emit tampered "{\\"paths_b64\\":\\"$(tr '\\0' '\\n' < "$W/changed.list" | head -c 4000 | b64)\\"}"
  exit 87
fi
${snapshot}${fetchHoldout}${container && harness ? launcherConfFragment() : ''}cd ${harness ? '"$H"' : '"$CODE"'} || exit 30
${exports}
OUT="$W/logs/${file}"
s=$(date +%s%N)
timeout -k 10 ${timeoutS} ${run} </dev/null >"$OUT.stdout" 2>"$OUT.stderr"; rc=$?
e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
${container ? 'docker rm -f "$CN" >/dev/null 2>&1 || true' : ''}
${isolated ? 'kill_users' : ''}
${o.holdout ? 'wipe' : ''}
to=false
if [ $rc -eq 124 ] || { [ $rc -eq 137 ] && [ $ms -ge $(( ${timeoutS} * 1000 )) ]; }; then to=true; fi
${result}
exit 0
`;
}

export interface Sentinel {
  kind: string;
  data: Record<string, unknown>;
}

export function parseSentinels(stdout: string): Sentinel[] {
  const out: Sentinel[] = [];
  for (const line of stdout.split('\n')) {
    if (!line.startsWith(SENTINEL + ' ')) continue;
    const rest = line.slice(SENTINEL.length + 1);
    const sp = rest.indexOf(' ');
    if (sp < 0) continue;
    try {
      const data = JSON.parse(rest.slice(sp + 1)) as unknown;
      if (data !== null && typeof data === 'object') out.push({ kind: rest.slice(0, sp), data: data as Record<string, unknown> });
    } catch {
      // truncated line
    }
  }
  return out;
}

/** stdout without sentinel lines (they are structured data, not log text). */
export function stripSentinels(stdout: string): string {
  return stdout
    .split('\n')
    .filter((l) => !l.startsWith(SENTINEL + ' '))
    .join('\n');
}

/** Where the holdout lives on the instance (inside the 0700 holdout directory) while a bench run uses it. */
export function localTracePath(layout: Layout, uri: string): string {
  const ext = extname(uri.split('?')[0] ?? '').replace(/[^A-Za-z0-9.]/g, '');
  return `${layout.work}/holdout/trace${ext}`;
}
