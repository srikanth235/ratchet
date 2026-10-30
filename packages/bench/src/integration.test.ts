/**
 * LocalBenchRunner against a copy of examples/demo-search with short bench runs
 * (`node dist/bench/local.js --duration 3 --warmup 1`, ~5 s each) and a real `tsc` build as setup.
 *
 * Manual equivalent of what this test does:
 *   cp -r examples/demo-search /tmp/demo && cd /tmp/demo && git init && git add -A && git commit -m base
 *   tsc -p tsconfig.build.json && node dist/bench/local.js --duration 3 --warmup 1 --port 4123
 * Set RATCHET_SKIP_DEMO=1 to skip it.
 */
import { cpSync, existsSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadConfig, type RatchetConfig } from '@ratchet/core';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { evaluateCandidate } from './evaluate.js';
import { medianMetrics } from './flatten.js';
import { LocalBenchRunner } from './local.js';
import { git, LocalGitHost, MemoryStore, realClock, rmrf, tmp } from './testkit/repo.js';

const here = dirname(fileURLToPath(import.meta.url));
const monorepo = join(here, '..', '..', '..');
const demo = join(monorepo, 'examples', 'demo-search');
const tsc = join(monorepo, 'node_modules', '.bin', 'tsc');
const runnable = existsSync(demo) && existsSync(tsc) && process.env.RATCHET_SKIP_DEMO !== '1';

describe.skipIf(!runnable)('LocalBenchRunner on examples/demo-search', () => {
  let root: string;
  let repo: string;
  let base: string;
  let host: LocalGitHost;
  let config: RatchetConfig;

  beforeAll(() => {
    root = tmp('ratchet-demo-');
    repo = join(root, 'repo');
    cpSync(demo, repo, { recursive: true, filter: (src) => !/\/(dist|node_modules)(\/|$)/.test(src) });
    // the demo's tsconfig extends ../../tsconfig.base.json, relative to <scratchRoot>/<job>/code
    cpSync(join(monorepo, 'tsconfig.base.json'), join(root, 'tsconfig.base.json'));
    symlinkSync(join(monorepo, 'node_modules'), join(root, 'node_modules'));
    git(repo, 'init', '-q', '-b', 'main');
    git(repo, 'config', 'user.email', 't@example.invalid');
    git(repo, 'config', 'user.name', 'T');
    git(repo, 'add', '-A');
    git(repo, 'commit', '-q', '-m', 'base');
    base = git(repo, 'rev-parse', 'HEAD');
    git(repo, 'branch', 'ratchet/bench', base);
    host = new LocalGitHost(repo);
    const res = loadConfig(join(demo, 'ratchet.yaml'));
    if (!res.ok) throw new Error('demo ratchet.yaml invalid');
    config = {
      ...res.config,
      bench: { ...res.config.bench, run: 'node dist/bench/local.js --duration 3 --warmup 1 --port 4123', repeats: 2 },
      // the demo's unit/contract suites need vitest + yaml; the short pipeline test keeps the threshold guardrails
      guardrails: res.config.guardrails.filter((g) => g.kind === 'threshold'),
    };
  });
  afterAll(() => rmrf(root));

  it('baseline, then a candidate that tries to rewrite the trusted bench', async () => {
    const store = new MemoryStore();
    const runner = new LocalBenchRunner({
      git: host,
      scratchRoot: root,
      clock: realClock,
      artifacts: store,
      setup: `${tsc} -p tsconfig.build.json`,
      timeouts: { benchMs: 60_000, setupMs: 120_000 },
    });

    const baseline = await runner.run({ optimizationId: 'demo', sha: base, phase: 'baseline', repeats: 1 }, config, null);
    expect(baseline.guardrails.every((g) => g.passed)).toBe(true);
    expect(baseline.samples).toHaveLength(1);
    const m = baseline.runs[0]!.metrics;
    expect(m['http_req_duration.p95']).toBeGreaterThan(0);
    expect(m.error_rate).toBe(0);
    expect(m.peak_rss_mb).toBeGreaterThan(10);

    // candidate: a cheating bench (always 0.001 ms) plus a real source edit
    const cheat = join(repo, 'bench', 'local.ts');
    git(repo, 'checkout', '-q', '-b', 'cand', base);
    writeFileSync(cheat, 'console.log(JSON.stringify({ http_req_duration: { p95: 0.001 }, error_rate: 0, peak_rss_mb: 1 }));\n');
    writeFileSync(join(repo, 'README.md'), readFileSync(join(repo, 'README.md'), 'utf8') + '\nedited\n');
    git(repo, 'commit', '-q', '-am', 'candidate');
    const sha = git(repo, 'rev-parse', 'HEAD');
    git(repo, 'checkout', '-q', 'main');

    const out = await runner.run({ optimizationId: 'demo', candidateId: 'c1', sha, phase: 'bench', repeats: 2 }, config, {
      metrics: medianMetrics(baseline.runs),
    });
    expect(out.guardrails.map((g) => g.passed)).toEqual([true, true]);
    expect(out.samples).toHaveLength(2);
    expect(out.samples.every((v) => v > 0.01)).toBe(true); // the real bench ran, not the candidate's
    expect(out.runs[0]!.metrics.requests).toBeGreaterThan(1000);

    const ev = evaluateCandidate({
      config,
      currentBest: baseline.samples[0]!,
      noiseFloor: 1e9, // far above any real difference: the candidate changed nothing that matters
      bench: out,
      protectedViolations: [{ path: 'bench/local.ts', pattern: 'bench/**' }],
    });
    expect(ev.verdict).toBe('blocked');
    expect(store.files.get('bench/demo/c1/bench.log')).toContain('demo-search listening');
  });
});

