import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { BenchJob, BenchRunner, RatchetConfig } from '@ratchet/core';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { BenchAbortedError, BenchError } from './errors.js';
import { LocalBenchRunner } from './local.js';
import { RemoteBenchRunner } from './remote.js';
import { LocalFakeCompute } from './testkit/compute.js';
import {
  alive,
  DirStore,
  instantClock,
  listDir,
  makeConfig,
  makeFixtureRepo,
  MemoryStore,
  realClock,
  type FixtureRepo,
} from './testkit/repo.js';

const STAGING_BUCKET = 'ratchet-test-artifacts';

type Opts = { setup?: string; benchMs?: number; guardrailMs?: number; env?: Record<string, string> };
interface Harness {
  runner: BenchRunner;
  store: MemoryStore;
  /** Remote only. */
  compute?: LocalFakeCompute;
  /** Local only: where job dirs are created. */
  scratch?: string;
}
type Factory = (fx: FixtureRepo, o?: Opts) => Harness;

const local: Factory = (fx, o = {}) => {
  const store = new MemoryStore();
  const scratch = join(fx.dir, 'scratch');
  const runner = new LocalBenchRunner({
    git: fx.host,
    scratchRoot: scratch,
    clock: realClock,
    artifacts: store,
    ...(o.setup ? { setup: o.setup } : {}),
    ...(o.env ? { env: o.env } : {}),
    timeouts: { ...(o.benchMs ? { benchMs: o.benchMs } : {}), ...(o.guardrailMs ? { guardrailMs: o.guardrailMs } : {}) },
  });
  return { runner, store, scratch };
};

const remote: Factory = (fx, o = {}) => {
  const store = new MemoryStore();
  const s3 = join(fx.dir, 's3');
  const compute = new LocalFakeCompute(join(fx.dir, 'ssm'), o.env ?? {}, s3);
  const runner = new RemoteBenchRunner({
    compute,
    git: fx.host,
    clock: instantClock,
    imageRef: 'ami-test',
    artifacts: store,
    stagingBucket: STAGING_BUCKET,
    stagingStore: new DirStore(join(s3, STAGING_BUCKET)),
    isolation: 'none', // scripts run as the developer here; the 'user' path is exercised with shims below
    workRoot: join(fx.dir, 'work'),
    ...(o.setup ? { setup: o.setup } : {}),
    timeouts: { ...(o.benchMs ? { benchMs: o.benchMs } : {}), ...(o.guardrailMs ? { guardrailMs: o.guardrailMs } : {}) },
  });
  return { runner, store, compute };
};

const job = (fx: FixtureRepo, sha: string, over: Partial<BenchJob> = {}): BenchJob => ({
  optimizationId: 'opt1',
  candidateId: 'cand1',
  sha,
  phase: 'bench',
  repeats: 3,
  ...over,
});

async function waitFor(pred: () => boolean, ms = 20_000): Promise<void> {
  const t0 = Date.now();
  while (!pred()) {
    if (Date.now() - t0 > ms) throw new Error('waitFor timed out');
    await new Promise((r) => setTimeout(r, 25));
  }
}

for (const [name, factory] of [
  ['LocalBenchRunner', local],
  ['RemoteBenchRunner (scripts run via local bash)', remote],
] as const) {
  describe(name, () => {
    let fx: FixtureRepo;
    beforeAll(() => {
      fx = makeFixtureRepo();
    });
    afterAll(() => fx.cleanup());

    const config = (o?: Parameters<typeof makeConfig>[0]) => makeConfig(o);

    it('runs the pipeline: guardrails pass, N runs, flattened metrics, log stored', async () => {
      const h = factory(fx);
      const sha = fx.candidate({ 'src/p95.txt': '80' });
      const progress: string[] = [];
      const out = await (h.runner as LocalBenchRunner).run(job(fx, sha), config(), { metrics: { peak_rss_mb: 100 } }, undefined, (p) =>
        progress.push(p.step === 'bench' ? `bench ${p.run}/${p.of}` : p.step === 'guardrails' ? `guardrails ${p.index}/${p.total}` : p.step),
      );
      expect(out.samples).toEqual([80, 80, 80]);
      expect(out.runs).toHaveLength(3);
      const m = out.runs[0]!.metrics;
      expect(m).toMatchObject({
        'http_req_duration.p95': 80,
        'http_req_duration.p50': 1,
        error_rate: 0,
        peak_rss_mb: 100,
        trace_set: 0,
        run: 1,
      });
      expect(m).not.toHaveProperty('label'); // strings and arrays are not metrics
      expect(m).not.toHaveProperty('arr');
      expect(out.runs.map((r) => r.metrics.run)).toEqual([1, 2, 3]);
      expect(out.guardrails.map((g) => [g.name, g.passed])).toEqual([
        ['node tests/check.mjs', true],
        ['error_rate < 0.001', true],
        ['peak_rss < 1.2x', true],
      ]);
      expect(out.finishedAt).toBeGreaterThanOrEqual(out.startedAt);
      expect(out.logsKey).toBe('bench/opt1/cand1/bench.log');
      const log = h.store.files.get('bench/opt1/cand1/bench.log')!;
      expect(log).toContain('some log line');
      expect(log).toMatch(/bench run 3\/3/);
      expect(progress).toEqual(['checkout', 'guardrails 1/1', 'bench 1/3', 'bench 2/3', 'bench 3/3', 'evaluate']);
    });

    it('stores the baseline log under baseline/', async () => {
      const h = factory(fx);
      const out = await h.runner.run(job(fx, fx.base, { phase: 'baseline', repeats: 1, candidateId: undefined }), config(), null);
      expect(out.logsKey).toBe('bench/opt1/baseline/baseline.log');
      // relative bounds are skipped (not failed) while establishing the baseline
      expect(out.guardrails.find((g) => g.name.startsWith('peak_rss'))).toMatchObject({ passed: true });
      expect(out.guardrails.every((g) => g.passed)).toBe(true);
    });

    it('blocks before the bench when a command guardrail fails', async () => {
      const marker = join(fx.dir, `marker-${name.length}-guard`);
      const h = factory(fx, { env: { MARKER: marker } });
      const sha = fx.candidate({ 'src/p95.txt': 'BROKEN' });
      const out = await h.runner.run(job(fx, sha), config(), null);
      expect(out.runs).toEqual([]);
      expect(out.samples).toEqual([]);
      expect(out.guardrails).toEqual([{ name: 'node tests/check.mjs', passed: false, detail: 'exited with status 1' }]);
      expect(existsSync(marker)).toBe(false); // the bench never started
      expect(h.store.files.get('bench/opt1/cand1/bench.log')).toContain('check failed: broken');
    });

    it('fails fast: later command guardrails are not run after the first failure', async () => {
      const marker = join(fx.dir, `marker-${name.length}-failfast`);
      const h = factory(fx);
      const out = await h.runner.run(
        job(fx, fx.base),
        config({ guardrails: ['- run: "exit 3"', `- run: "touch ${marker}"`] }),
        null,
      );
      expect(out.guardrails).toHaveLength(1);
      expect(out.guardrails[0]).toMatchObject({ passed: false, detail: 'exited with status 3' });
      expect(existsSync(marker)).toBe(false);
    });

    it('ignores candidate edits to protected bench files (they come from bench.ref)', async () => {
      const h = factory(fx);
      const sha = fx.candidate({
        'bench/bench.mjs': 'console.log(JSON.stringify({ http_req_duration: { p95: 1 }, error_rate: 0, peak_rss_mb: 1 }));',
        'bench/extra.js': 'evil',
        'tests/check.mjs': 'process.exit(0)',
        'tests/new.mjs': 'evil',
        'src/p95.txt': '90',
      });
      const out = await h.runner.run(
        job(fx, sha),
        config({ guardrails: ['- run: "test ! -e bench/extra.js && test ! -e tests/new.mjs && ! grep -q process.exit.0. tests/check.mjs"'] }),
        null,
      );
      // 90 = the real bench ran against the candidate's src; bench.mjs said 1
      expect(out.samples).toEqual([90, 90, 90]);
      expect(out.guardrails.every((g) => g.passed)).toBe(true);
    });

    it('does not follow a candidate symlink that replaces a protected directory', async () => {
      const h = factory(fx);
      const sha = fx.candidate({
        bench: { symlink: 'evil' },
        'evil/bench.mjs': 'console.log(JSON.stringify({ http_req_duration: { p95: 1 } }));',
        'src/p95.txt': '95',
      });
      const out = await h.runner.run(job(fx, sha, { repeats: 1 }), config({ guardrails: [] }), null);
      expect(out.samples).toEqual([95]);
    });

    it('can keep candidate-only protected files when pruning is off (local)', async () => {
      if (name !== 'LocalBenchRunner') return;
      const store = new MemoryStore();
      const runner = new LocalBenchRunner({ git: fx.host, scratchRoot: join(fx.dir, 'scratch2'), clock: realClock, artifacts: store, pruneExtraProtected: false });
      const sha = fx.candidate({ 'tests/new.mjs': 'x' });
      const out = await runner.run(job(fx, sha, { repeats: 1 }), config({ guardrails: ['- run: test -e tests/new.mjs'] }), null);
      expect(out.guardrails[0]).toMatchObject({ passed: true });
    });

    it('records one value per repeat (job.repeats) with their metrics', async () => {
      const marker = join(fx.dir, `marker-${name.length}-repeats`);
      const h = factory(fx, { env: { MARKER: marker, JITTER: '1' } });
      const out = await h.runner.run(job(fx, fx.base, { repeats: 4 }), config({ guardrails: [] }), null);
      expect(out.runs).toHaveLength(4);
      expect(out.samples.map((x) => Number(x.toFixed(1)))).toEqual([100, 100.1, 100.2, 100.3]);
      expect(readFileSync(marker, 'utf8').trim().split('\n')).toHaveLength(4);
    });

    it('evaluates threshold guardrails on run medians, relative ones against the baseline metrics', async () => {
      const h = factory(fx);
      const cfg = config({ guardrails: ['- error_rate: "< 0.001"', '- peak_rss: "< 1.2x"'] });
      const bad = fx.candidate({ 'src/rss.txt': '150', 'src/err.txt': '0.01' });
      const out = await h.runner.run(job(fx, bad, { repeats: 2 }), cfg, { metrics: { peak_rss_mb: 100 } });
      expect(out.guardrails).toHaveLength(2);
      expect(out.guardrails[0]).toMatchObject({ name: 'error_rate < 0.001', passed: false });
      expect(out.guardrails[1]).toMatchObject({ name: 'peak_rss < 1.2x', passed: false });
      expect(out.guardrails[1]!.detail).toContain('peak_rss_mb = 150');
      expect(out.guardrails[1]!.detail).toContain('baseline 100');

      const ok = fx.candidate({ 'src/rss.txt': '110' });
      const out2 = await h.runner.run(job(fx, ok, { repeats: 2 }), cfg, { metrics: { peak_rss_mb: 100 } });
      expect(out2.guardrails.every((g) => g.passed)).toBe(true);

      // no baseline: a relative bound cannot be evaluated, and fails closed
      const out3 = await h.runner.run(job(fx, ok, { repeats: 1 }), cfg, null);
      expect(out3.guardrails[1]).toMatchObject({ passed: false });
      expect(out3.guardrails[1]!.detail).toMatch(/baseline/);
    });

    it('fails a threshold guardrail whose metric the bench does not report', async () => {
      const h = factory(fx);
      const out = await h.runner.run(job(fx, fx.base, { repeats: 1 }), config({ guardrails: ['- p99_latency: "< 5"'] }), null);
      expect(out.guardrails[0]).toMatchObject({ passed: false });
      expect(out.guardrails[0]!.detail).toMatch(/not reported/);
    });

    it('passes RATCHET_TRACE to confirm runs only, and skips command guardrails there', async () => {
      const h = factory(fx);
      const traceFile = join(fx.dir, 's3', 'sealed', 'trace.jsonl');
      mkdirSync(join(fx.dir, 's3', 'sealed'), { recursive: true });
      writeFileSync(traceFile, '0123456789');
      const cfg = config({ confirmTrace: name === 'LocalBenchRunner' ? traceFile : 's3://sealed/trace.jsonl' });
      const confirm = await h.runner.run(job(fx, fx.base, { phase: 'confirm', repeats: 2 }), cfg, { metrics: { peak_rss_mb: 100 } });
      expect(confirm.runs.map((r) => [r.metrics.trace_set, r.metrics.trace_bytes, r.metrics.phase_is_confirm])).toEqual([
        [1, 10, 1],
        [1, 10, 1],
      ]);
      // threshold guardrails only; the command guardrail passed in the bench phase
      expect(confirm.guardrails.map((g) => g.name)).toEqual(['error_rate < 0.001', 'peak_rss < 1.2x']);

      // job.trace overrides the config; a bench-phase job never gets the trace
      writeFileSync(join(fx.dir, 's3', 'sealed', 'other.jsonl'), 'abc');
      const override = await h.runner.run(
        job(fx, fx.base, { phase: 'confirm', repeats: 1, trace: name === 'LocalBenchRunner' ? join(fx.dir, 's3', 'sealed', 'other.jsonl') : 's3://sealed/other.jsonl' }),
        cfg,
        { metrics: { peak_rss_mb: 100 } },
      );
      expect(override.runs[0]!.metrics.trace_bytes).toBe(3);
      const bench = await h.runner.run(job(fx, fx.base, { repeats: 1, trace: traceFile }), cfg, { metrics: { peak_rss_mb: 100 } });
      expect(bench.runs[0]!.metrics.trace_set).toBe(0);
    });

    it('refuses a confirm job without a trace', async () => {
      const h = factory(fx);
      await expect(h.runner.run(job(fx, fx.base, { phase: 'confirm' }), config(), null)).rejects.toThrow(BenchError);
    });

    it('aborts a running bench and kills its processes', async () => {
      const marker = join(fx.dir, `marker-${name.length}-abort`);
      const h = factory(fx, { env: { MARKER: marker } });
      const sha = fx.candidate({ 'src/hang.txt': '1' });
      const ac = new AbortController();
      const t0 = Date.now();
      const p = h.runner.run(job(fx, sha), config(), null, ac.signal);
      p.catch(() => undefined);
      await waitFor(() => existsSync(marker));
      ac.abort();
      await expect(p).rejects.toBeInstanceOf(BenchAbortedError);
      await expect(p).rejects.toMatchObject({ name: 'AbortError' });
      expect(Date.now() - t0).toBeLessThan(15_000);
      const pids = readFileSync(marker, 'utf8').trim().split('\n').map(Number);
      await waitFor(() => pids.every((pid) => !alive(pid)), 5000);
      if (h.compute) expect(h.compute.terminated).toHaveLength(1); // instance terminated on abort
      if (h.scratch) expect(listDir(h.scratch)).toEqual([]); // job dir cleaned up
      expect(h.store.files.get('bench/opt1/cand1/bench.log')).toBeDefined(); // the log is still stored
    });

    it('rejects immediately when the signal is already aborted', async () => {
      const h = factory(fx);
      const ac = new AbortController();
      ac.abort();
      await expect(h.runner.run(job(fx, fx.base), config(), null, ac.signal)).rejects.toBeInstanceOf(BenchAbortedError);
      if (h.compute) expect(h.compute.launched).toHaveLength(0);
    });

    it('turns a bench timeout into a failed bench.run guardrail', async () => {
      const h = factory(fx, { benchMs: 1000 });
      const sha = fx.candidate({ 'src/hang.txt': '1' });
      const out = await h.runner.run(job(fx, sha), config({ guardrails: [] }), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails).toHaveLength(1);
      expect(out.guardrails[0]).toMatchObject({ name: 'bench.run', passed: false });
      expect(out.guardrails[0]!.detail).toMatch(/run 1\/3: timed out after 1000ms/);
    });

    it('turns a command guardrail timeout into a failed guardrail', async () => {
      const h = factory(fx, { guardrailMs: 1000 });
      const out = await h.runner.run(job(fx, fx.base), config({ guardrails: ['- run: sleep 30'] }), null);
      expect(out.guardrails).toEqual([{ name: 'sleep 30', passed: false, detail: 'timed out after 1000ms' }]);
    });

    it('reports bench failures: non-zero exit, no JSON, missing metric', async () => {
      const h = factory(fx);
      const cfg = (run: string) => config({ guardrails: [], run });
      const exit2 = await h.runner.run(job(fx, fx.base, { repeats: 2 }), cfg('echo hi; exit 2'), null);
      expect(exit2.guardrails[0]).toMatchObject({ name: 'bench.run', passed: false, detail: 'run 1/2: exited with status 2' });
      const noJson = await h.runner.run(job(fx, fx.base, { repeats: 2 }), cfg('echo hi'), null);
      expect(noJson.guardrails[0]!.detail).toMatch(/no JSON object/);
      const missing = await h.runner.run(job(fx, fx.base, { repeats: 2 }), cfg('echo \'{"a":1}\''), null);
      expect(missing.guardrails[0]!.detail).toMatch(/objective metric "http_req_duration.p95" not found/);
      expect(missing.runs).toEqual([]);
    });

    it('keeps the completed runs when a later run fails', async () => {
      const marker = join(fx.dir, `marker-${name.length}-partial`);
      const h = factory(fx, { env: { MARKER: marker } });
      const run = `n=$(wc -l < ${marker} 2>/dev/null || echo 0); echo x >> ${marker}; if [ "$n" -ge 1 ]; then exit 9; fi; echo '{"http_req_duration":{"p95":7}}'`;
      const out = await h.runner.run(job(fx, fx.base, { repeats: 3 }), config({ guardrails: ['- error_rate: "< 0.001"'], run }), null);
      expect(out.samples).toEqual([7]);
      // thresholds are not evaluated for an incomplete bench
      expect(out.guardrails).toEqual([{ name: 'bench.run', passed: false, detail: 'run 2/3: exited with status 9' }]);
    });

    it('runs setup before the guardrails and blocks when it fails', async () => {
      const ok = factory(fx, { setup: 'echo 77 > src/p95.txt' });
      const out = await ok.runner.run(job(fx, fx.base, { repeats: 1 }), config({ guardrails: [] }), null);
      expect(out.samples).toEqual([77]);

      const bad = factory(fx, { setup: 'echo setup-broke >&2; exit 4' });
      const marker = join(fx.dir, `marker-${name.length}-setup`);
      const out2 = await bad.runner.run(job(fx, fx.base), config(), null);
      expect(out2.runs).toEqual([]);
      expect(out2.guardrails).toEqual([{ name: 'setup', passed: false, detail: 'exited with status 4' }]);
      expect(existsSync(marker)).toBe(false);
      expect(bad.store.files.get('bench/opt1/cand1/bench.log')).toContain('setup-broke');
    });

    it('blocks a candidate whose setup/build rewrites trusted bench files, and restores them', async () => {
      const h = factory(fx, { setup: 'echo "console.log(JSON.stringify({http_req_duration:{p95:1}}))" > bench/bench.mjs' });
      const out = await h.runner.run(job(fx, fx.base, { repeats: 1 }), config({ guardrails: ['- run: "true"'] }), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails).toHaveLength(1);
      expect(out.guardrails[0]).toMatchObject({ name: 'protected files intact', passed: false });
      expect(out.guardrails[0]!.detail).toContain('bench/bench.mjs');
    });

    it('protects the evaluator and ratchet.yaml with no protected: list at all (derived from evaluator.run)', async () => {
      const h = factory(fx, { setup: 'echo "console.log(JSON.stringify({http_req_duration:{p95:1}}))" > bench/bench.mjs' });
      const out = await h.runner.run(job(fx, fx.base, { repeats: 1 }), config({ guardrails: ['- run: "true"'], protectedGlobs: [] }), null);
      expect(out.runs).toEqual([]);
      expect(out.guardrails[0]).toMatchObject({ name: 'protected files intact', passed: false });
      expect(out.guardrails[0]!.detail).toContain('bench/bench.mjs');
    });

    it('fails with a BenchError when bench.ref has no protected files', async () => {
      const h = factory(fx);
      await expect(h.runner.run(job(fx, fx.base), config({ protectedGlobs: ['nothing/**'], run: 'sh ./score.sh' }), null)).rejects.toThrow(/contains no files matching/);
    });

    it('fails with a BenchError when the candidate cannot be checked out', async () => {
      const h = factory(fx);
      await expect(h.runner.run(job(fx, 'deadbeef'.repeat(5)), config(), null)).rejects.toThrow(BenchError);
      if (h.compute) expect(h.compute.terminated).toHaveLength(1);
      if (h.scratch) expect(listDir(h.scratch)).toEqual([]);
    });
  });
}

describe('RemoteBenchRunner specifics', () => {
  let fx: FixtureRepo;
  beforeAll(() => {
    fx = makeFixtureRepo();
  });
  afterAll(() => fx.cleanup());

  const mk = (over: Partial<ConstructorParameters<typeof RemoteBenchRunner>[0]> = {}, extraEnv: Record<string, string> = {}) => {
    const s3 = join(fx.dir, 's3');
    const compute = new LocalFakeCompute(join(fx.dir, 'ssm'), extraEnv, s3);
    const staging = new DirStore(join(s3, STAGING_BUCKET));
    const slept: number[] = [];
    const runner = new RemoteBenchRunner({
      compute,
      git: fx.host,
      clock: { now: () => Date.now(), sleep: async (ms) => void slept.push(ms) },
      imageRef: 'ami-test',
      stagingBucket: STAGING_BUCKET,
      stagingStore: staging,
      isolation: 'none',
      workRoot: join(fx.dir, 'work'),
      ...over,
    });
    return { compute, runner, slept, staging };
  };
  const cfg = () => makeConfig({ guardrails: [] });

  it('launches one pinned instance per job with the right purpose and always terminates it', async () => {
    const { compute, runner } = mk();
    await runner.run(job(fx, fx.base, { repeats: 1 }), cfg(), null);
    await runner.run(job(fx, fx.base, { repeats: 1, phase: 'confirm', trace: join(fx.dir, 'trace.txt') }), cfg(), null).catch(() => undefined);
    expect(compute.launched.map((l) => [l.instanceType, l.purpose, l.imageRef, l.optimizationId])).toEqual([
      ['c7i.2xlarge', 'bench', 'ami-test', 'opt1'],
      ['c7i.2xlarge', 'confirm', 'ami-test', 'opt1'],
    ]);
    expect(compute.terminated).toEqual(['i-fake1', 'i-fake2']);
  });

  it('terminates the instance even when a step throws (bad ref)', async () => {
    const { compute, runner } = mk();
    await expect(runner.run(job(fx, 'nonexistent-ref'), cfg(), null)).rejects.toThrow(/cannot resolve candidate/);
    expect(compute.terminated).toEqual(['i-fake1']);
  });

  it('waits for the instance to register before the first command', async () => {
    const { compute, runner, slept } = mk({ readyDelayMs: 7 });
    compute.notReadyFor = 2;
    const out = await runner.run(job(fx, fx.base, { repeats: 1 }), cfg(), null);
    expect(out.samples).toEqual([100]);
    expect(slept).toEqual([7, 7]);
  });

  it('gives up when the instance never becomes ready, and still terminates it', async () => {
    const { compute, runner } = mk({ readyAttempts: 3 });
    compute.notReadyFor = 99;
    await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/never became ready/);
    expect(compute.terminated).toHaveLength(1);
  });

  it('reports (but survives) a failing terminate', async () => {
    const warnings: string[] = [];
    const { compute, runner } = mk({ onWarning: (m) => warnings.push(m) });
    compute.failTerminate = true;
    const out = await runner.run(job(fx, fx.base, { repeats: 1 }), cfg(), null);
    expect(out.samples).toEqual([100]);
    expect(warnings[0]).toMatch(/FAILED to terminate bench instance i-fake1/);
  });

  it('sends one script per step and keeps each output within the SSM limit', async () => {
    const { compute, runner } = mk();
    const c = makeConfig({ guardrails: ['- run: node tests/check.mjs', '- run: "true"'] });
    await runner.run(job(fx, fx.base, { repeats: 2 }), c, null);
    // ping, checkout, 2 guardrails, 2 bench runs
    expect(compute.commands).toHaveLength(6);
    // each is wrapped so it runs under bash even if SSM uses sh
    for (const cmd of compute.commands) expect(cmd.script).toMatch(/^f=\$\(mktemp\)\ncat > "\$f" <<'__RATCHET_STEP_EOF__'/);
    const checkout = compute.commands[1]!.script;
    expect(checkout).toContain('aws s3 cp');
    expect(checkout).toContain(`s3://${STAGING_BUCKET}/bench-inputs/opt1/`);
    expect(checkout).toContain(fx.base);
  });

  it('pre-stages the candidate and bench.ref in the artifacts bucket and never contacts a git remote from the instance', async () => {
    const { compute, runner, staging } = mk();
    await runner.run(job(fx, fx.base, { repeats: 1 }), cfg(), null);
    expect(staging.puts).toHaveLength(2);
    expect(staging.puts[0]).toMatch(/^bench-inputs\/opt1\/[0-9a-f-]{36}\/code\.tgz$/);
    expect(staging.puts[1]).toMatch(/^bench-inputs\/opt1\/[0-9a-f-]{36}\/bench\.tgz$/);
    for (const c of compute.commands) {
      expect(c.script).not.toMatch(/git (--git-dir|fetch|clone|ls-remote)|github\.com|https?:\/\//);
      expect(c.script).not.toContain(fx.repo); // no remote url, no token
    }
  });

  it('refuses to run a job without a staging bucket or store (fail closed, no GitHub fallback)', async () => {
    const { compute, runner } = mk({ stagingBucket: undefined });
    await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/needs stagingBucket/);
    expect(compute.commands).toHaveLength(0);
    expect(compute.terminated).toHaveLength(1);
    const noStore = mk({ stagingStore: undefined });
    await expect(noStore.runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/needs stagingBucket/);
  });

  it('rejects an optimization id that is not [a-z0-9-] before anything is staged (S3 prefix injection)', async () => {
    for (const bad of ['*', '?', '/', '..', 'a/b', '../opt2', 'opt1/..', 'a*', 'Opt1', '']) {
      const { compute, runner, staging } = mk();
      await expect(runner.run(job(fx, fx.base, { optimizationId: bad }), cfg(), null)).rejects.toThrow(/must match \[a-z0-9-\]/);
      expect(staging.puts).toHaveLength(0);
      expect(compute.commands).toHaveLength(0);
    }
  });

  it('rejects a staging bucket that is not a plain bucket name', async () => {
    for (const bad of ['a/b', 'bucket*', '..', "x'; rm -rf /", 'A_B']) {
      const { runner, staging } = mk({ stagingBucket: bad });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/not a valid bucket name/);
      expect(staging.puts).toHaveLength(0);
    }
  });

  it('detects a staged archive that was swapped after upload (sha256 check on the instance)', async () => {
    const { runner, staging } = mk();
    const put = staging.put.bind(staging);
    staging.put = async (key, body) => {
      await put(key, body);
      if (key.endsWith('code.tgz')) writeFileSync(join(fx.dir, 's3', STAGING_BUCKET, key), 'not the archive');
    };
    await expect(runner.run(job(fx, fx.base, { repeats: 1 }), cfg(), null)).rejects.toThrow(/sha256 mismatch/);
  });

  describe("isolation: 'user' (OS tools shimmed; the real users/iptables/sudo are image work, see bench.md)", () => {
    const userRunner = (shims: Record<string, string> = {}, over: Partial<ConstructorParameters<typeof RemoteBenchRunner>[0]> = {}) => {
      const s3 = join(fx.dir, 's3');
      const compute = new LocalFakeCompute(join(fx.dir, 'ssm'), {}, s3);
      const log = join(fx.dir, `shim-${Math.random().toString(36).slice(2)}.log`);
      const defaults: Record<string, string> = {
        id: `[ "$2" = ratchet-bench ] || [ "$2" = ratchet-holdout ] && exit 0; exec /usr/bin/id "$@"`,
        iptables: `echo "iptables $*" >> ${log}; exit 0`,
        setpriv: `echo "setpriv $*" >> ${log}; if [[ "$*" == *"/dev/tcp/169.254.169.254"* ]]; then exit 1; fi; if [[ "$*" == *"--reuid=ratchet-bench"* && "$*" == *"holdout/.probe"* ]]; then exit 1; fi; while [ "$1" != -- ]; do shift; done; shift; exec "$@"`,
        pkill: `echo "pkill $*" >> ${log}; exit 0`,
        chown: `echo "chown $*" >> ${log}; exit 0`,
        'ratchet-candidate': `echo "launcher $*" >> ${log}; if [ "$*" = "id -un" ]; then echo ratchet-bench; exit 0; fi; exec env "$@"`,
        ...shims,
      };
      for (const [n, body] of Object.entries(defaults)) compute.addShim(n, body);
      const runner = new RemoteBenchRunner({
        compute,
        git: fx.host,
        clock: { now: () => Date.now(), sleep: async () => undefined },
        imageRef: 'ami-test',
        stagingBucket: STAGING_BUCKET,
        stagingStore: new DirStore(join(s3, STAGING_BUCKET)),
        isolation: 'user',
        candidateLauncher: compute.shimPath('ratchet-candidate'),
        workRoot: join(fx.dir, 'work'),
        ...over,
      });
      return { runner, compute, log: () => (existsSync(log) ? readFileSync(log, 'utf8') : '') };
    };
    const holdoutFile = () => {
      const root = join(fx.dir, 'work');
      return listDir(root)
        .map((d) => join(root, d, 'holdout'))
        .filter((d) => existsSync(d))
        .flatMap((d) => listDir(d).filter((f) => f.startsWith('trace')).map((f) => join(d, f)));
    };
    const seedTrace = (body = '{"requests":[1,2,3]}') => {
      mkdirSync(join(fx.dir, 's3', STAGING_BUCKET), { recursive: true });
      writeFileSync(join(fx.dir, 's3', STAGING_BUCKET, 'trace.json'), body);
      return `s3://${STAGING_BUCKET}/trace.json`;
    };

    it('verifies both users, the IMDS block, the holdout permissions and the launcher, then splits the principals', async () => {
      const { runner, compute, log } = userRunner();
      const out = await runner.run(job(fx, fx.base, { repeats: 1 }), makeConfig({ guardrails: ['- run: "true"'] }), null);
      expect(out.samples).toEqual([100]);
      const l = log();
      for (const u of ['ratchet-bench', 'ratchet-holdout']) {
        expect(l).toContain(`iptables -C OUTPUT -m owner --uid-owner ${u} -d 169.254.169.254 -j REJECT`);
      }
      expect(l).toContain('chown -R ratchet-bench:');
      expect(l).toContain('chown -R ratchet-holdout:');
      // setup / guardrails: candidate user, no new privileges. Bench harness: the OTHER user, from the snapshot
      expect(l).toContain('setpriv --reuid=ratchet-bench --regid=ratchet-bench --clear-groups --no-new-privs -- env -u AWS_ACCESS_KEY_ID');
      expect(l).toMatch(/setpriv --reuid=ratchet-holdout --regid=ratchet-holdout --clear-groups -- env -u AWS_ACCESS_KEY_ID/);
      // before and after every step: both users' processes are killed
      expect((l.match(/pkill -KILL -u ratchet-bench/g) ?? []).length).toBeGreaterThanOrEqual(4);
      expect((l.match(/pkill -KILL -u ratchet-holdout/g) ?? []).length).toBeGreaterThanOrEqual(4);
      const step = compute.commands.map((c) => c.script).find((s) => s.includes('bench.mjs'))!;
      expect(step).toContain('env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN');
      expect(step).toContain('RATCHET_CANDIDATE_EXEC=');
    });

    it('runs the harness from a root-owned read-only snapshot of the built tree, not from the candidate-writable tree', async () => {
      const { runner, compute, log } = userRunner();
      const bench = 'node -e "console.log(JSON.stringify({http_req_duration:{p95:1},cwd_is_snapshot:process.cwd().endsWith(\'/harness\')?1:0,src_visible:require(\'fs\').existsSync(\'src/p95.txt\')?1:0}))"';
      const out = await runner.run(job(fx, fx.base, { repeats: 1 }), makeConfig({ guardrails: [], run: bench }), null);
      expect(out.runs[0]!.metrics).toMatchObject({ cwd_is_snapshot: 1, src_visible: 1 });
      expect(log()).toMatch(/chown -hR root:root .*harness\.tmp/);
      const bs = compute.commands.map((c) => c.script).filter((s) => s.includes('snapshot()'));
      expect(bs).toHaveLength(1); // one bench run, one script; the snapshot is made once and reused by later runs
    });

    it('reuses the snapshot for later runs instead of copying the tree again', async () => {
      const { runner, compute } = userRunner();
      await runner.run(job(fx, fx.base, { repeats: 2 }), makeConfig({ guardrails: [] }), null);
      const scripts = compute.commands.map((c) => c.script).filter((s) => s.includes('snapshot()'));
      expect(scripts).toHaveLength(2);
      expect(scripts.every((s) => s.includes('[ -d "$H" ] || snapshot'))).toBe(true);
    });

    it('refuses to run the candidate when the image has no bench user', async () => {
      const { runner, compute } = userRunner({ id: 'exit 1' });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/bench user ratchet-bench does not exist/);
      expect(compute.terminated).toHaveLength(1);
      expect(compute.commands.every((c) => !c.script.includes('bench.mjs'))).toBe(true);
    });

    it('refuses when the image has no holdout user', async () => {
      const { runner } = userRunner({ id: `[ "$2" = ratchet-bench ] && exit 0; exit 1` });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/holdout user ratchet-holdout does not exist/);
    });

    it('refuses to run the candidate when the IMDS block rule is missing', async () => {
      const { runner } = userRunner({ iptables: 'exit 1' });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/IMDS is not blocked/);
    });

    it('refuses when the IMDS rule exists for the bench user but not for the holdout user', async () => {
      const { runner } = userRunner({ iptables: 'case "$*" in *ratchet-holdout*) exit 1;; esac; exit 0' });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/IMDS is not blocked for ratchet-holdout/);
    });

    it('refuses to run the candidate when instance metadata is still reachable as the bench user', async () => {
      const { runner } = userRunner({ setpriv: 'exit 0' });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/metadata is reachable/);
    });

    it('refuses when the holdout directory could be read by the bench user', async () => {
      // a setpriv that lets the bench user open anything
      const { runner } = userRunner({ setpriv: `if [[ "$*" == *"/dev/tcp/169.254.169.254"* ]]; then exit 1; fi; while [ "$1" != -- ]; do shift; done; shift; exec "$@"` });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/holdout directory is readable by ratchet-bench/);
    });

    it('refuses when the launcher does not run commands as the bench user', async () => {
      const { runner } = userRunner({ 'ratchet-candidate': 'echo root' });
      await expect(runner.run(job(fx, fx.base), cfg(), null)).rejects.toThrow(/candidate launcher .* does not run commands as ratchet-bench/);
    });

    describe('the holdout (confirm)', () => {
      const confirmJob = (uri: string) => job(fx, fx.base, { repeats: 2, phase: 'confirm', trace: uri });

      it('is fetched by root only after setup and guardrails have finished, and is gone again after every run', async () => {
        const uri = seedTrace();
        const { runner, compute, log } = userRunner();
        // setup and the command guardrail FAIL if the holdout exists while they run (cwd is <work>/code)
        const gone = 'test -z "$(ls ../holdout 2>/dev/null)"';
        const out = await runner.run(confirmJob(uri), makeConfig({ guardrails: [`- run: '${gone}'`] }), null, undefined, undefined);
        expect(out.guardrails.find((g) => g.name === 'bench.run')).toBeUndefined();
        expect(out.samples).toEqual([100, 100]);
        expect(out.runs.map((r) => [r.metrics.trace_set, r.metrics.trace_bytes])).toEqual([[1, 20], [1, 20]]);
        expect(holdoutFile()).toEqual([]); // wiped after the last run
        // order of remote steps: ping/checkout, then every bench step; the sealed location is only in bench steps
        const scripts = compute.commands.map((c) => c.script);
        const withTrace = scripts.map((s, i) => [i, s.includes('trace.json')] as const).filter(([, has]) => has).map(([i]) => i);
        const firstBench = scripts.findIndex((s) => s.includes('bench.mjs'));
        expect(withTrace.length).toBe(2);
        expect(withTrace.every((i) => i >= firstBench)).toBe(true);
        for (const s of scripts.slice(0, firstBench)) expect(s).not.toContain('aws s3 cp --only-show-errors "$URI"');
        // permissions: fetched as root (no setpriv before the fetch), handed to the harness user alone, mode 0600
        const benchScript = scripts[firstBench]!;
        expect(benchScript.indexOf('aws s3 cp')).toBeGreaterThan(-1);
        expect(benchScript.indexOf('chown "$HU:" "$TRACE"')).toBeGreaterThan(benchScript.indexOf('aws s3 cp'));
        expect(benchScript.indexOf('chmod 600 "$TRACE"')).toBeGreaterThan(benchScript.indexOf('chown "$HU:" "$TRACE"'));
        expect(benchScript.indexOf('setpriv')).toBeGreaterThan(benchScript.indexOf('chmod 600 "$TRACE"'));
        expect(benchScript).not.toMatch(/chmod 4[0-9]0|chgrp/);
        expect(log()).toMatch(/chown ratchet-holdout: .*holdout\/trace\.json/);
        expect(compute.terminated).toHaveLength(1);
      });

      it('keeps RATCHET_TRACE and the sealed location out of everything the candidate user runs', async () => {
        const uri = seedTrace();
        const { runner, compute } = userRunner({}, { setup: 'true' });
        await runner.run(confirmJob(uri), makeConfig({ guardrails: [] }), null);
        const candidateSteps = compute.commands.map((c) => c.script).filter((s) => s.includes('--no-new-privs'));
        expect(candidateSteps.length).toBeGreaterThanOrEqual(2); // checkout (probes) and setup
        for (const s of candidateSteps) {
          expect(s).not.toContain('RATCHET_TRACE');
          expect(s).not.toContain('trace.json');
        }
      });

      it('never passes the holdout to candidate processes: the launcher starts them with a clean environment', async () => {
        const uri = seedTrace();
        const { runner } = userRunner({
          // like the real launcher: `env -i`
          'ratchet-candidate': `if [ "$*" = "id -un" ]; then echo ratchet-bench; exit 0; fi; exec env -i PATH="$PATH" "$@"`,
        });
        // single-quoted for the shell, so $RATCHET_TRACE reaches the inner sh unexpanded
        const probe = [
          'const {execFileSync}=require("child_process");',
          'const out=execFileSync(process.env.RATCHET_CANDIDATE_EXEC,["sh","-c","test -n \\"$RATCHET_TRACE\\" && echo 1 || echo 0"],{encoding:"utf8"}).trim();',
          'console.log(JSON.stringify({http_req_duration:{p95:1},leaked:Number(out),harness_has_trace:process.env.RATCHET_TRACE?1:0}));',
        ].join('');
        const out = await runner.run(confirmJob(uri), makeConfig({ guardrails: [], run: `node -e '${probe}'` }), null);
        expect(out.runs[0]!.metrics).toMatchObject({ harness_has_trace: 1, leaked: 0 });
      });

      it('is fetched once per run, and the harness has it at that run', async () => {
        const uri = seedTrace('0123456789');
        const { runner } = userRunner();
        const out = await runner.run(confirmJob(uri), makeConfig({ guardrails: [] }), null);
        expect(out.runs.map((r) => r.metrics.trace_bytes)).toEqual([10, 10]);
      });

      it('is wiped when the harness crashes or times out', async () => {
        const uri = seedTrace();
        const { runner } = userRunner({}, { timeouts: { benchMs: 1500 } });
        const hang = 'node -e "setTimeout(()=>{}, 60000)"';
        const out = await runner.run(confirmJob(uri), makeConfig({ guardrails: [], run: hang }), null);
        expect(out.guardrails.some((g) => g.name === 'bench.run' && !g.passed)).toBe(true);
        expect(holdoutFile()).toEqual([]);
        const { runner: r2 } = userRunner();
        const crash = 'exit 3';
        const out2 = await r2.run(confirmJob(uri), makeConfig({ guardrails: [], run: crash }), null);
        expect(out2.guardrails.some((g) => g.name === 'bench.run' && !g.passed)).toBe(true);
        expect(holdoutFile()).toEqual([]);
      });

      it('turns a failed fetch into an infrastructure error, not a candidate failure', async () => {
        const { runner, compute } = userRunner();
        await expect(runner.run(confirmJob(`s3://${STAGING_BUCKET}/missing.json`), makeConfig({ guardrails: [] }), null)).rejects.toThrow(/fetching the holdout .* failed/);
        expect(compute.terminated).toHaveLength(1);
      });

      it('does not write the output of a holdout run to the stored log', async () => {
        const uri = seedTrace();
        const store = new MemoryStore();
        const { runner: r } = userRunner({}, { artifacts: store });
        const noisy = `node -e "console.error('SECRET-STDERR'); console.log('SECRET-STDOUT'); console.log(JSON.stringify({http_req_duration:{p95:2}}))"`;
        const out = await r.run(confirmJob(uri), makeConfig({ guardrails: [], run: noisy }), null);
        expect(out.samples).toEqual([2, 2]);
        const log = store.files.get(out.logsKey!)!;
        expect(log).not.toContain('SECRET-STDERR');
        expect(log).not.toContain('SECRET-STDOUT');
        expect(log).toMatch(/holdout: 20 bytes fetched/);
        // a bench-phase run of the same command does keep its tails (debuggability)
        const out2 = await r.run(job(fx, fx.base, { repeats: 1, candidateId: 'c2' }), makeConfig({ guardrails: [], run: noisy }), null);
        expect(store.files.get(out2.logsKey!)).toContain('SECRET-STDERR');
      });
    });

    describe('bench and baseline phases', () => {
      it('never contain the sealed location or a fetch, even if a trace is passed in the job', async () => {
        const uri = seedTrace();
        const { runner, compute } = userRunner();
        const out = await runner.run(job(fx, fx.base, { repeats: 2, trace: uri }), makeConfig({ guardrails: [] }), null);
        expect(out.runs.map((r) => r.metrics.trace_set)).toEqual([0, 0]);
        for (const c of compute.commands) {
          expect(c.script).not.toContain('trace.json');
          expect(c.script).not.toContain('aws s3 cp --only-show-errors "$URI"');
          expect(c.script).not.toContain('RATCHET_TRACE');
        }
        expect(compute.launched.map((l) => l.purpose)).toEqual(['bench']);
      });
    });
  });

  it('parses a very chatty bench because only the stdout tail travels', async () => {
    const { runner } = mk();
    const noisy = `node -e "for (let i=0;i<5000;i++) console.log('noise line '+i); console.log(JSON.stringify({http_req_duration:{p95:3}}))"`;
    const out = await runner.run(job(fx, fx.base, { repeats: 1 }), makeConfig({ guardrails: [], run: noisy }), null);
    expect(out.samples).toEqual([3]);
  });

  it('rejects refs that are not plain git refs before sending anything', async () => {
    const { compute, runner } = mk();
    await expect(runner.run(job(fx, '--upload-pack=evil'), cfg(), null)).rejects.toThrow(/not a plain git ref/);
    expect(compute.terminated).toHaveLength(1);
  });
});

describe('LocalBenchRunner specifics', () => {
  let fx: FixtureRepo;
  beforeAll(() => {
    fx = makeFixtureRepo();
  });
  afterAll(() => fx.cleanup());

  it('checks out the candidate and config.bench.ref into a fresh dir under scratchRoot and cleans up', async () => {
    const scratch = join(fx.dir, 'scr');
    const runner = new LocalBenchRunner({ git: fx.host, scratchRoot: scratch, clock: realClock });
    const sha = fx.candidate({ 'src/p95.txt': '60' });
    const out = await runner.run(job(fx, sha, { repeats: 1 }), makeConfig(), { metrics: { peak_rss_mb: 100 } });
    expect(out.samples).toEqual([60]);
    expect(fx.host.checkouts.slice(-2).map((c) => c.ref)).toEqual([sha, 'ratchet/bench']);
    expect(fx.host.checkouts.slice(-2).every((c) => c.dir.startsWith(scratch + '/'))).toBe(true);
    expect(listDir(scratch)).toEqual([]);
  });

  it('keepWorkdir leaves the job directory for debugging', async () => {
    const scratch = join(fx.dir, 'scr-keep');
    const runner = new LocalBenchRunner({ git: fx.host, scratchRoot: scratch, clock: realClock, keepWorkdir: true });
    await runner.run(job(fx, fx.base, { repeats: 1 }), makeConfig({ guardrails: [] }), null);
    expect(listDir(scratch)).toHaveLength(1);
    expect(existsSync(join(scratch, listDir(scratch)[0]!, 'code', 'bench', 'bench.mjs'))).toBe(true);
  });
});
