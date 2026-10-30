import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { matchesGlob } from '@ratchet/core';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { runProcess } from './exec.js';
import { flattenNumeric, medianMetrics, resolveMetric } from './flatten.js';
import { BenchLog } from './log.js';
import { applyOverlay, listFiles, restoreProtected } from './overlay.js';
import { evaluateThresholds, logKey, parseBenchOutput } from './pipeline.js';
import { BENCH_USER, CANDIDATE_LAUNCHER, checkoutScript, HOLDOUT_USER, protectedPcre, sq, stepScript, wrapStep } from './remote-scripts.js';
import { alive, realClock, rmrf, tmp } from './testkit/repo.js';

describe('flatten', () => {
  it('flattens numeric leaves to dotted keys, skipping arrays, strings, booleans, non-finite', () => {
    expect(flattenNumeric({ a: { b: 1, c: { d: 2 } }, e: 3, s: 'x', t: true, n: null, arr: [1], inf: Infinity })).toEqual({ 'a.b': 1, 'a.c.d': 2, e: 3 });
  });
  it('medianMetrics aggregates per key over the runs that report it', () => {
    expect(medianMetrics([{ metrics: { a: 1, b: 10 } }, { metrics: { a: 3 } }, { metrics: { a: 2, b: 20 } }])).toEqual({ a: 2, b: 15 });
  });
  it('resolveMetric prefers the exact key, then a unit suffix', () => {
    expect(resolveMetric({ peak_rss_mb: 5 }, 'peak_rss')).toEqual({ key: 'peak_rss_mb', value: 5 });
    expect(resolveMetric({ peak_rss: 1, peak_rss_mb: 5 }, 'peak_rss')).toEqual({ key: 'peak_rss', value: 1 });
    expect(resolveMetric({ x: 1 }, 'peak_rss')).toBeUndefined();
    expect(resolveMetric({}, 'toString')).toBeUndefined();
  });
});

describe('parseBenchOutput / evaluateThresholds', () => {
  it('uses the last JSON object, the objective by path, and flattens everything', () => {
    const r = parseBenchOutput('log\n{"x":1}\nmore\n{"lat":{"p95":4.5},"rps":10,"note":"hi"}\n', 'lat.p95');
    expect(r).toEqual({ ok: true, value: 4.5, metrics: { 'lat.p95': 4.5, rps: 10 } });
  });
  it('explains what is wrong', () => {
    expect(parseBenchOutput('nope', 'a')).toMatchObject({ ok: false });
    expect(parseBenchOutput('{"a":"x"}', 'a')).toMatchObject({ ok: false, error: expect.stringContaining('not a finite number') });
  });
  it('evaluates against run medians (not means or maxima)', () => {
    const g = [{ kind: 'threshold' as const, metric: 'error_rate', op: '<' as const, value: 0.01, relative: false, raw: '< 0.01' }];
    const runs = [0.5, 0.001, 0.002].map((e) => ({ value: 1, metrics: { error_rate: e }, durationMs: 1 }));
    expect(evaluateThresholds(g, runs, null, 'bench')[0]).toMatchObject({ passed: true }); // median 0.002
  });
});

describe('logKey', () => {
  it('follows bench/<opt>/<candidate|baseline>/<phase>.log and sanitizes', () => {
    expect(logKey({ optimizationId: 'o', candidateId: 'c1', sha: 's', phase: 'confirm', repeats: 1 })).toBe('bench/o/c1/confirm.log');
    expect(logKey({ optimizationId: 'o', sha: 'abc', phase: 'bench', repeats: 1 })).toBe('bench/o/abc/bench.log');
    expect(logKey({ optimizationId: 'o', candidateId: 'x', sha: 's', phase: 'baseline', repeats: 1 })).toBe('bench/o/baseline/baseline.log');
    expect(logKey({ optimizationId: '../o', candidateId: '../../x', sha: 's', phase: 'bench', repeats: 1 })).toBe('bench/.._o/.._.._x/bench.log');
  });
});

describe('BenchLog', () => {
  it('keeps head and tail when over the cap', () => {
    const log = new BenchLog(realClock, 100);
    log.write('H'.repeat(50) + 'M'.repeat(1000) + 'T'.repeat(50));
    const s = log.toString();
    expect(s.startsWith('H'.repeat(50))).toBe(true);
    expect(s.endsWith('T'.repeat(50))).toBe(true);
    expect(s).toContain('characters truncated');
    expect(s.length).toBeLessThan(300);
  });
});

describe('runProcess', () => {
  it('captures stdout, streams both, reports exit codes', async () => {
    const seen: string[] = [];
    const r = await runProcess('echo out; echo err >&2; exit 3', { cwd: '/', onData: (c, s) => seen.push(`${s}:${c.trim()}`) });
    expect(r).toMatchObject({ exitCode: 3, stdout: 'out\n', timedOut: false, aborted: false });
    expect(seen.sort()).toEqual(['stderr:err', 'stdout:out']);
  });
  it('kills the whole process group on timeout, including grandchildren', async () => {
    const dir = tmp();
    const r = await runProcess(`sh -c 'echo $$ > ${dir}/pid; sleep 30' & wait`, { cwd: dir, timeoutMs: 500 });
    expect(r.timedOut).toBe(true);
    const pid = Number(readFileSync(join(dir, 'pid'), 'utf8'));
    await new Promise((res) => setTimeout(res, 100));
    expect(alive(pid)).toBe(false);
    rmrf(dir);
  });
  it('clears stragglers left behind by a successful command', async () => {
    const dir = tmp();
    const r = await runProcess(`(sleep 30 & echo $! > ${dir}/pid) ; echo done`, { cwd: dir });
    expect(r.exitCode).toBe(0);
    await new Promise((res) => setTimeout(res, 100));
    expect(alive(Number(readFileSync(join(dir, 'pid'), 'utf8')))).toBe(false);
    rmrf(dir);
  });
  it('reports a missing cwd as a spawn error rather than throwing', async () => {
    const r = await runProcess('true', { cwd: '/definitely/not/here' });
    expect(r.spawnError).toBeDefined();
  });
});

describe('overlay', () => {
  let dir: string;
  beforeAll(() => {
    dir = tmp();
  });
  afterAll(() => rmrf(dir));

  const put = (root: string, rel: string, body: string) => {
    mkdirSync(join(root, rel, '..'), { recursive: true });
    writeFileSync(join(root, rel), body);
  };

  it('never writes through a candidate symlink to outside the checkout', async () => {
    const code = join(dir, 'code');
    const bench = join(dir, 'bench');
    const outside = join(dir, 'outside');
    mkdirSync(outside);
    writeFileSync(join(outside, 'precious.txt'), 'keep');
    put(bench, 'bench/precious.txt', 'trusted');
    put(code, 'src/a.txt', 'a');
    symlinkSync(outside, join(code, 'bench'));
    const res = await applyOverlay({ codeDir: code, benchDir: bench, globs: ['bench/**'] });
    expect(res.copied).toEqual(['bench/precious.txt']);
    expect(readFileSync(join(outside, 'precious.txt'), 'utf8')).toBe('keep');
    expect(readFileSync(join(code, 'bench/precious.txt'), 'utf8')).toBe('trusted');
  });

  it('restoreProtected detects and repairs edits, deletions and symlink swaps', async () => {
    const code = join(dir, 'code2');
    const bench = join(dir, 'bench2');
    put(bench, 'bench/a.js', 'A');
    put(bench, 'bench/sub/b.js', 'B');
    put(bench, 'tests/t.js', 'T');
    mkdirSync(code);
    const globs = ['bench/**', 'tests/**'];
    const { copied } = await applyOverlay({ codeDir: code, benchDir: bench, globs });
    expect(await restoreProtected({ codeDir: code, benchDir: bench, files: copied })).toEqual([]);
    writeFileSync(join(code, 'bench/a.js'), 'evil');
    execFileSync('rm', ['-r', join(code, 'bench/sub')]);
    execFileSync('rm', ['-r', join(code, 'tests')]);
    symlinkSync(dir, join(code, 'tests'));
    expect((await restoreProtected({ codeDir: code, benchDir: bench, files: copied })).sort()).toEqual(['bench/a.js', 'bench/sub/b.js', 'tests/t.js']);
    expect(readFileSync(join(code, 'bench/a.js'), 'utf8')).toBe('A');
    expect(existsSync(join(code, 'bench/sub/b.js'))).toBe(true);
    expect(await restoreProtected({ codeDir: code, benchDir: bench, files: copied })).toEqual([]);
  });

  it('skips .git and node_modules when listing', async () => {
    const root = join(dir, 'list');
    put(root, 'a.txt', '');
    put(root, 'node_modules/x/y.js', '');
    put(root, '.git/config', '');
    put(root, 'd/e.txt', '');
    expect(await listFiles(root)).toEqual(['a.txt', 'd/e.txt']);
  });
});

describe('remote script helpers', () => {
  it('sq quotes anything', () => {
    expect(sq("it's")).toBe(`'it'\\''s'`);
    expect(execFileSync('bash', ['-c', `printf %s ${sq(`a'b"c$d\`e\\f`)}`], { encoding: 'utf8' })).toBe(`a'b"c$d\`e\\f`);
  });
  it('wrapStep refuses a body that contains the delimiter', () => {
    expect(() => wrapStep('x\n__RATCHET_STEP_EOF__\ny')).toThrow();
  });
  it('protectedPcre (grep -P) agrees with core matchesGlob on the same paths', () => {
    const globs = ['bench/**', 'ratchet.yaml', 'tests/contract/**', '**/*.snap', 'docs/*.md', 'src/{a,b}/x?.ts', './lead/', 'SOLUTIONS.md', '[ab]c/**'];
    const paths = [
      'bench/k6/search.js', 'bench', 'benchmark/x', 'ratchet.yaml', 'sub/ratchet.yaml', 'tests/contract/a/b.ts', 'tests/unit/a.ts',
      'x/y/z.snap', 'z.snap', 'docs/a.md', 'docs/sub/a.md', 'src/a/x1.ts', 'src/b/x12.ts', 'src/c/x1.ts', 'lead/f', 'lead', 'SOLUTIONS.md',
      'ac/q', 'bc/q/r', 'cc/q', 'a.b/c', 'weird name/bench/x', '.hidden/bench/x', 'bench/.dot',
    ];
    for (const re of [protectedPcre(globs)]) {
      const input = paths.join('\0') + '\0';
      const out = execFileSync('grep', ['-zP', re], { input, encoding: 'utf8' }).split('\0').filter(Boolean);
      const expected = paths.filter((p) => globs.some((g) => matchesGlob(p, g)));
      expect(out).toEqual(expected);
    }
    // no globs: matches nothing
    expect(() => execFileSync('grep', ['-zP', protectedPcre([])], { input: 'a\0', encoding: 'utf8' })).toThrow();
  });
});

describe('generated step scripts: candidate / holdout split', () => {
  const layout = { work: '/var/tmp/work/job1' };
  const base = { layout, name: 'bench run 1/3', command: 'k6 run bench/k6/search.js', timeoutMs: 60_000, env: { RATCHET_PHASE: 'confirm' } };
  const staged = (n: string) => ({ uri: `s3://artifacts/bench-inputs/opt/job/${n}.tgz`, sha256: 'a'.repeat(64), commit: 'b'.repeat(40) });
  const HOLDOUT_URI = 's3://sealed/holdout/trace.json';
  const confirm = stepScript({ ...base, kind: 'bench', holdout: { uri: HOLDOUT_URI } });
  const at = (s: string, needle: string) => {
    const i = s.indexOf(needle);
    expect(i, `script contains ${needle}`).toBeGreaterThan(-1);
    return i;
  };
  const syntax = (s: string) => expect(() => execFileSync('bash', ['-n'], { input: s })).not.toThrow();

  it('every variant is syntactically valid bash', () => {
    syntax(confirm);
    syntax(stepScript({ ...base, kind: 'bench' }));
    syntax(stepScript({ ...base, kind: 'bench', isolation: 'none', holdout: { uri: HOLDOUT_URI } }));
    syntax(stepScript({ ...base, kind: 'setup' }));
    syntax(stepScript({ ...base, kind: 'guardrail', isolation: 'none' }));
    syntax(checkoutScript({ layout, code: staged('code'), bench: staged('bench'), globs: ['bench/**'], prune: true, isolation: 'user' }));
  });

  it('confirm bench step order: kill leftovers, verify protected files, snapshot, THEN fetch the holdout, run the harness, kill, wipe', () => {
    const order = ['kill_users\n', 'apply check', 'snapshot()', 'aws s3 cp', 'chown "$HU:" "$TRACE"', 'chmod 600 "$TRACE"', 'export RATCHET_TRACE="$TRACE"', 'setpriv --reuid="$HU"', '\nwipe\n'];
    const positions = order.map((n) => at(confirm, n));
    expect(positions).toEqual([...positions].sort((a, b) => a - b));
    expect(confirm).toContain('trap wipe EXIT'); // also wiped when the script is killed or fails midway
    expect(at(confirm, 'trap wipe EXIT')).toBeLessThan(at(confirm, 'aws s3 cp'));
  });

  it('the holdout lives in the 0700 holdout dir under the job dir and is never owned by or readable to the candidate user', () => {
    expect(confirm).toContain(`TRACE='/var/tmp/work/job1/holdout/trace.json'`);
    expect(confirm.match(/chown [^\n]*TRACE[^\n]*/g)).toEqual(['chown "$HU:" "$TRACE" || exit 42']);
    expect(confirm.match(/chmod [^\n]*TRACE[^\n]*/g)).toEqual(['chmod 600 "$TRACE" || exit 42']);
    expect(confirm).not.toContain('chgrp');
    expect(confirm).toContain('shred -u');
  });

  it('the harness runs as the holdout user without a bench-user step, from the snapshot, and gets the launcher', () => {
    expect(confirm).toContain('cd "$H"');
    expect(confirm).toMatch(/setpriv --reuid="\$HU" --regid="\$HU" --clear-groups -- env -u AWS_ACCESS_KEY_ID .*RATCHET_CANDIDATE_EXEC='\/usr\/local\/bin\/ratchet-candidate' bash -c 'k6 run bench\/k6\/search\.js'/);
    // sudo needs setuid: no --no-new-privs for the harness (trusted code); candidate steps keep it
    expect(confirm).not.toContain('--no-new-privs');
    const setup = stepScript({ ...base, kind: 'setup', command: 'pnpm build' });
    expect(setup).toContain('setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs');
    expect(setup).toContain('cd "$CODE"');
    expect(setup).not.toContain('$HU" --regid');
  });

  it('the snapshot is root-owned, read-only, setuid-free and made once', () => {
    expect(confirm).toContain('cp -a');
    expect(confirm).toContain('-type f -perm /6000 -exec chmod ug-s');
    expect(confirm).toContain('chown -hR root:root');
    expect(confirm).toContain('chmod -R go-w,a+rX');
    expect(confirm).toContain('[ -d "$H" ] || snapshot');
  });

  it('only a confirm bench step carries the holdout location and fetch; nothing else does', () => {
    const others = [
      stepScript({ ...base, kind: 'bench' }),
      stepScript({ ...base, kind: 'setup' }),
      stepScript({ ...base, kind: 'guardrail' }),
      stepScript({ ...base, kind: 'bench', isolation: 'none' }),
      checkoutScript({ layout, code: staged('code'), bench: staged('bench'), globs: ['bench/**'], prune: true, isolation: 'user' }),
      checkoutScript({ layout, code: staged('code'), bench: staged('bench'), globs: ['bench/**'], prune: true, isolation: 'none' }),
    ];
    for (const s of others) {
      expect(s).not.toContain('RATCHET_TRACE');
      expect(s).not.toContain('sealed');
      expect(s).not.toContain('trace.json');
      expect(s).not.toContain('shred');
    }
    // the only s3 reads in the checkout are the two staged archives (the bench-inputs prefix of the artifacts bucket)
    const co = others[4]!;
    expect([...co.matchAll(/s3:\/\/[^'" ]+/g)].map((m) => m[0])).toEqual([staged('code').uri, staged('bench').uri]);
  });

  it('refuses a holdout on a non-bench step, and a caller-supplied RATCHET_TRACE', () => {
    expect(() => stepScript({ ...base, kind: 'setup', holdout: { uri: HOLDOUT_URI } })).toThrow(/only ever given to bench steps/);
    expect(() => stepScript({ ...base, kind: 'guardrail', holdout: { uri: HOLDOUT_URI } })).toThrow(/only ever given/);
    expect(() => stepScript({ ...base, kind: 'bench', env: { RATCHET_TRACE: '/x' } })).toThrow(/RATCHET_TRACE is set by the step script/);
  });

  it('quotes a hostile holdout uri and a hostile launcher path', () => {
    const s = stepScript({ ...base, kind: 'bench', holdout: { uri: "s3://b/k'; touch /pwned; '.json" }, candidateLauncher: "/x'; touch /pwned2; '" });
    syntax(s);
    expect(s).not.toMatch(/^touch \/pwned/m);
    expect(s).toContain(`URI=${sq("s3://b/k'; touch /pwned; '.json")}`);
    expect(s).toContain(`RATCHET_CANDIDATE_EXEC=${sq("/x'; touch /pwned2; '")}`);
  });

  it('the checkout verifies both users, both IMDS rules, the holdout permissions and the launcher', () => {
    const co = checkoutScript({ layout, code: staged('code'), bench: staged('bench'), globs: ['bench/**'], prune: true, isolation: 'user' });
    for (const needle of [
      'id -u "$BU"',
      'id -u "$HU"',
      'iptables -C OUTPUT -m owner --uid-owner "$u" -d 169.254.169.254 -j REJECT',
      'the holdout directory is readable by $BU',
      'chmod 700 "$W/holdout"',
      'does not run commands as $BU',
    ]) {
      expect(co).toContain(needle);
    }
    expect(co).toContain("LAUNCHER='/usr/local/bin/ratchet-candidate'");
    // the sealed bucket is not touched while checking out
    expect(co).not.toMatch(/sealed|holdout\/trace/);
  });

  it('constants line up with the instance user-data (packages/aws): users and launcher path', () => {
    expect([BENCH_USER, HOLDOUT_USER, CANDIDATE_LAUNCHER]).toEqual(['ratchet-bench', 'ratchet-holdout', '/usr/local/bin/ratchet-candidate']);
  });
});
