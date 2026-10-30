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

export function checkoutScript(o: {
  layout: Layout;
  repoUrl: string;
  sha: string;
  benchRef: string;
  globs: readonly string[];
  prune: boolean;
}): string {
  assertSafeRef(o.sha, 'candidate sha');
  assertSafeRef(o.benchRef, 'bench.ref');
  return `${prelude(o.layout)}${overlayFns}
URL=${sq(o.repoUrl)}
RE=${sq(protectedPcre(o.globs))}
rm -rf -- "\${W:?}"; mkdir -p "$CODE" "$STAGE" "$W/logs" || exit 10
git init -q --bare "$G" || exit 11
resolve() {
  local ref="$1" c
  if git --git-dir="$G" fetch -q --no-tags "$URL" "$ref" 2>>"$W/logs/fetch.err" && c=$(git --git-dir="$G" rev-parse --verify -q 'FETCH_HEAD^{commit}'); then
    printf '%s\\n' "$c"; return 0
  fi
  git --git-dir="$G" fetch -q --no-tags "$URL" '+refs/heads/*:refs/remotes/origin/*' '+refs/tags/*:refs/tags/*' 2>>"$W/logs/fetch.err" || return 1
  git --git-dir="$G" rev-parse --verify -q "$ref^{commit}" || git --git-dir="$G" rev-parse --verify -q "refs/remotes/origin/$ref^{commit}"
}
CODE_SHA=$(resolve ${sq(o.sha)}) || { echo "cannot resolve candidate ${o.sha}" >&2; tail -c 2000 "$W/logs/fetch.err" >&2; exit 20; }
BENCH_SHA=$(resolve ${sq(o.benchRef)}) || { echo "cannot resolve bench.ref ${o.benchRef}" >&2; tail -c 2000 "$W/logs/fetch.err" >&2; exit 21; }
git --git-dir="$G" archive "$CODE_SHA" | tar -x -C "$CODE" || exit 22
git --git-dir="$G" archive "$BENCH_SHA" | tar -x -C "$STAGE" || exit 23
git --git-dir="$G" ls-tree -r -z --name-only "$BENCH_SHA" | grep -zP "$RE" > "$W/bench.match" || true
git --git-dir="$G" ls-tree -r -z --name-only "$CODE_SHA" | grep -zP "$RE" > "$W/code.match" || true
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
emit checkout "{\\"code\\":\\"$CODE_SHA\\",\\"bench\\":\\"$BENCH_SHA\\",\\"removed\\":$removed,\\"copied\\":$copied}"
`;
}

/** Download the hidden trace from the sealed bucket onto the instance (mode 0600, outside the code tree). */
export function traceScript(o: { layout: Layout; uri: string; localPath: string }): string {
  return `${prelude(o.layout)}
mkdir -p "$W/trace" || exit 40
URI=${sq(o.uri)}
DST=${sq(o.localPath)}
case "$URI" in
  s3://*) aws s3 cp --only-show-errors "$URI" "$DST" || exit 41 ;;
  file://*) cp -- "\${URI#file://}" "$DST" || exit 41 ;;
  *) cp -- "$URI" "$DST" || exit 41 ;;
esac
chmod 600 "$DST"
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
  return `${prelude(o.layout)}${overlayFns}
apply check
if [ -s "$W/changed.list" ]; then
  emit tampered "{\\"paths_b64\\":\\"$(tr '\\0' '\\n' < "$W/changed.list" | head -c 4000 | b64)\\"}"
  exit 87
fi
cd "$CODE" || exit 30
${exports}
OUT="$W/logs/${file}"
s=$(date +%s%N)
timeout -k 10 ${timeoutS} bash -c ${sq(o.command)} </dev/null >"$OUT.stdout" 2>"$OUT.stderr"; rc=$?
e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
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
