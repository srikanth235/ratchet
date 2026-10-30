import { extname } from 'node:path';
import { globToRegExp } from '@ratchet/core';
import { BenchError } from './errors.js';

/**
 * Shell scripts for the remote (bench instance) pipeline. One script per step; every script is
 * wrapped by `wrapStep` so it runs under bash even when the SSM document uses `sh`.
 *
 * Results travel on stdout as sentinel lines `@@RATCHET@@ <kind> <json>`, because SSM returns at
 * most ~24,000 characters of output per command: the bench JSON is sent as the last 12,000 bytes of
 * stdout, base64 encoded, and each bench run is its own command so the budget is per run.
 */

export const SENTINEL = '@@RATCHET@@';
/** Unprivileged OS user that runs setup, guardrails and the bench (= candidate code). Created and IMDS-blocked by the instance's user-data (packages/aws `benchUserData`). */
export const BENCH_USER = 'ratchet-bench';

/** Where one archive was staged (see stage.ts) and the digest the instance must see. */
export interface StagedArchive {
  /** `s3://<artifacts bucket>/bench-inputs/<opt>/<job>/<name>.tgz` */
  uri: string;
  sha256: string;
  /** Full commit sha the archive was made from. */
  commit: string;
}

/** `user`: steps run as BENCH_USER with IMDS blocked (default, fail-closed). `none`: single-user, for local tests only. */
export type Isolation = 'user' | 'none';
const HEREDOC = '__RATCHET_STEP_EOF__';

/** POSIX single-quote a string. */
export function sq(s: string): string {
  if (s.includes('\0')) throw new BenchError('NUL byte in a shell argument');
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

/** Make a bash script safe to hand to a shell that may not be bash: write it to a file and run it with bash. */
export function wrapStep(body: string): string {
  if (body.includes(HEREDOC)) throw new BenchError('script contains the step delimiter');
  return `f=$(mktemp)\ncat > "$f" <<'${HEREDOC}'\n${body}\n${HEREDOC}\nbash "$f"\nrc=$?\nrm -f "$f"\nexit $rc\n`;
}

const SAFE_REF = /^[A-Za-z0-9][A-Za-z0-9._\/@{}~^+-]*$/;

/** A sha / ref is interpolated into git commands; keep it to plain ref characters and never leading '-'. */
export function assertSafeRef(ref: string, what: string): string {
  if (!SAFE_REF.test(ref) || ref.includes('..')) throw new BenchError(`${what} ${JSON.stringify(ref)} is not a plain git ref or sha`);
  return ref;
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
CODE="$W/code"; STAGE="$W/stage"; G="$W/repo.git"
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

/** Checks that the instance really has the unprivileged user and the IMDS block; refuses to continue otherwise. */
const isolationCheck = (isolation: Isolation): string =>
  isolation === 'none'
    ? ''
    : `BU=${sq(BENCH_USER)}
id -u "$BU" >/dev/null 2>&1 || { echo "bench user $BU does not exist on this image" >&2; exit 12; }
iptables -C OUTPUT -m owner --uid-owner "$BU" -d 169.254.169.254 -j REJECT >/dev/null 2>&1 || { echo "IMDS is not blocked for $BU (iptables owner rule missing)" >&2; exit 13; }
if setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo "instance metadata is reachable as $BU" >&2; exit 14; fi
mkdir -p "$W/home" "$W/tmp" || exit 15
chown -R "$BU:" "$CODE" "$W/home" "$W/tmp" || exit 16
chmod 755 "$W"
`;

/**
 * Fetches the pre-staged candidate and bench.ref archives from the artifacts bucket (through the S3
 * gateway endpoint; the bench subnet has no other way out), verifies their digests, applies the
 * protected overlay, and - with `isolation: 'user'` - verifies the unprivileged user and the IMDS
 * block and hands the code tree to that user. No git, no GitHub, no credentials in the script.
 */
export function checkoutScript(o: {
  layout: Layout;
  code: StagedArchive;
  bench: StagedArchive;
  globs: readonly string[];
  prune: boolean;
  isolation: Isolation;
}): string {
  for (const a of [o.code, o.bench]) {
    assertSafeRef(a.commit, 'staged commit');
    if (!/^s3:\/\/[a-z0-9][a-z0-9.-]*\/[A-Za-z0-9._\/-]+$/.test(a.uri) || a.uri.includes('..')) throw new BenchError(`staged archive uri ${JSON.stringify(a.uri)} is not a plain s3:// uri`);
    if (!/^[0-9a-f]{64}$/.test(a.sha256)) throw new BenchError('staged archive digest is not a sha256 hex string');
  }
  return `${prelude(o.layout)}${overlayFns}
RE=${sq(protectedPcre(o.globs))}
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
${isolationCheck(o.isolation)}emit checkout "{\\"code\\":\\"${o.code.commit}\\",\\"bench\\":\\"${o.bench.commit}\\",\\"removed\\":$removed,\\"copied\\":$copied,\\"isolation\\":\\"${o.isolation}\\"}"
`;
}

/**
 * Download the hidden trace from the sealed bucket onto the instance. This pre-step runs as root (the
 * SSM agent's user), the only user with the instance-profile credentials; it is never run as the bench
 * user. With `isolation: 'user'` the file is root:BENCH_USER 0440, so the bench process can read it
 * but not replace it, and nothing the bench user runs can reach the credentials that fetched it (IMDS
 * is blocked for that uid).
 */
export function traceScript(o: { layout: Layout; uri: string; localPath: string; isolation: Isolation }): string {
  const perms =
    o.isolation === 'none'
      ? 'chmod 600 "$DST"'
      : `chgrp ${sq(BENCH_USER)} "$W/trace" "$DST" || exit 42\nchmod 750 "$W/trace" || exit 42\nchmod 440 "$DST" || exit 42`;
  return `${prelude(o.layout)}
mkdir -p "$W/trace" || exit 40
URI=${sq(o.uri)}
DST=${sq(o.localPath)}
case "$URI" in
  s3://*) aws s3 cp --only-show-errors "$URI" "$DST" || exit 41 ;;
  file://*) cp -- "\${URI#file://}" "$DST" || exit 41 ;;
  *) cp -- "$URI" "$DST" || exit 41 ;;
esac
${perms}
emit trace "{\\"bytes\\":$(wc -c < "$DST" | tr -d ' ')}"
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
}

/**
 * Setup, guardrail and bench steps. Before running the command the script verifies that the
 * protected files still equal bench.ref's; if not it restores them, reports `tampered`, and exits
 * 87 without running the command.
 */
export function stepScript(o: StepScriptInput): string {
  const timeoutS = Math.max(1, Math.ceil(o.timeoutMs / 1000));
  const file = o.name.replace(/[^A-Za-z0-9._-]/g, '_').slice(0, 60);
  for (const k of Object.keys(o.env)) if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(k)) throw new BenchError(`invalid environment variable name ${k}`);
  const exports = Object.entries(o.env)
    .map(([k, v]) => `export ${k}=${sq(v)}`)
    .join('\n');
  const result =
    o.kind === 'bench'
      ? `emit run "{\\"exit\\":$rc,\\"ms\\":$ms,\\"timedOut\\":$to,\\"stdout_b64\\":\\"$(tail -c 12000 "$OUT.stdout" | b64)\\"}"
echo "--- stderr tail ---"; tail -c 3000 "$OUT.stderr"`
      : `emit step "{\\"exit\\":$rc,\\"ms\\":$ms,\\"timedOut\\":$to}"
echo "--- output tail ---"; { tail -c 4000 "$OUT.stdout"; tail -c 2000 "$OUT.stderr"; }`;
  const isolated = (o.isolation ?? 'user') === 'user';
  // The command runs as an unprivileged user without new privileges; everything it left running is
  // killed before the next root-side step touches the tree (no racing the overlay), and AWS credential
  // variables are scrubbed even though a stock SSM environment has none.
  const run = isolated
    ? `setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN -u AWS_PROFILE HOME="$W/home" TMPDIR="$W/tmp" USER="$BU" LOGNAME="$BU" bash -c ${sq(o.command)}`
    : `bash -c ${sq(o.command)}`;
  return `${prelude(o.layout)}${overlayFns}
${isolated ? `BU=${sq(BENCH_USER)}\nkill_bench() { pkill -KILL -u "$BU" 2>/dev/null || true; }\nkill_bench` : ''}
apply check
if [ -s "$W/changed.list" ]; then
  emit tampered "{\\"paths_b64\\":\\"$(tr '\\0' '\\n' < "$W/changed.list" | head -c 4000 | b64)\\"}"
  exit 87
fi
cd "$CODE" || exit 30
${exports}
OUT="$W/logs/${file}"
s=$(date +%s%N)
timeout -k 10 ${timeoutS} ${run} </dev/null >"$OUT.stdout" 2>"$OUT.stderr"; rc=$?
e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
${isolated ? 'kill_bench' : ''}
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

/** Where the trace lives on the instance when it was downloaded from `uri`. */
export function localTracePath(layout: Layout, uri: string): string {
  const ext = extname(uri.split('?')[0] ?? '').replace(/[^A-Za-z0-9.]/g, '');
  return `${layout.work}/trace/trace${ext}`;
}
