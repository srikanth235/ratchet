import { describe, expect, it } from 'vitest';
import { INSTALL_FILES, effectiveProtected, evaluatorDirsFromRun } from './protected-set.js';
import { findProtectedViolations } from './protected.js';

const base = { evaluator: { run: 'node other/script.js' } };

describe('evaluatorDirsFromRun', () => {
  it('finds bench/ and eval/ directories referenced by the command', () => {
    expect(evaluatorDirsFromRun('k6 run bench/k6/search.js')).toEqual(['bench/**']);
    expect(evaluatorDirsFromRun('node eval/backtest.mjs')).toEqual(['eval/**']);
    expect(evaluatorDirsFromRun('node ./bench/run.mjs && node eval/check.mjs')).toEqual(['bench/**', 'eval/**']);
  });
  it('treats a built copy as the evaluator and also protects the sources it is built from', () => {
    expect(evaluatorDirsFromRun('node dist/bench/hyperfine.js --runs 10')).toEqual(['dist/bench/**', 'bench/**']);
  });
  it('sees paths inside shell expansions and quotes', () => {
    const run = 'node dist/bench/hyperfine.js --mix "${RATCHET_TRACE:-bench/invocations.json}"';
    expect(evaluatorDirsFromRun(run)).toEqual(['dist/bench/**', 'bench/**']);
  });
  it('ignores words that merely look similar, absolute paths and paths that leave the repo', () => {
    expect(evaluatorDirsFromRun('node benchmark/run.js')).toEqual([]);
    expect(evaluatorDirsFromRun('node my-evaluation/run.js')).toEqual([]);
    expect(evaluatorDirsFromRun('node /opt/bench/run.js')).toEqual([]);
    expect(evaluatorDirsFromRun('node ../bench/run.js')).toEqual([]);
    expect(evaluatorDirsFromRun('make eval')).toEqual([]);
  });
});

describe('effectiveProtected', () => {
  it('always protects the config and the lockfiles / install config, at any depth', () => {
    const p = effectiveProtected(base);
    expect(p.globs).toContain('ratchet.yaml');
    for (const f of INSTALL_FILES) expect(p.globs).toContain(`**/${f}`);
    const hits = findProtectedViolations(
      ['pnpm-lock.yaml', 'packages/api/package-lock.json', '.npmrc', 'pnpm-workspace.yaml', '.pnpmfile.cjs', 'npm-shrinkwrap.json', 'src/index.ts'],
      p.globs,
    );
    expect(hits.map((h) => h.path)).toEqual([
      'pnpm-lock.yaml',
      'packages/api/package-lock.json',
      '.npmrc',
      'pnpm-workspace.yaml',
      '.pnpmfile.cjs',
      'npm-shrinkwrap.json',
    ]);
  });

  it('uses the config path in use', () => {
    expect(effectiveProtected(base, { configPath: 'ops/ratchet.yaml' }).globs[0]).toBe('ops/ratchet.yaml');
    expect(effectiveProtected(base, { configPath: './ratchet.yaml' }).globs[0]).toBe('ratchet.yaml');
  });

  it('protects what evaluator.run references under bench/ or eval/', () => {
    const p = effectiveProtected({ evaluator: { run: 'k6 run bench/k6/search.js' } });
    expect(findProtectedViolations(['bench/k6/search.js', 'bench/query-mix.json', 'src/app.ts'], p.globs).map((v) => v.path)).toEqual([
      'bench/k6/search.js',
      'bench/query-mix.json',
    ]);
  });

  it("protects a template's evaluator files, even when run is not given", () => {
    const p = effectiveProtected({ evaluator: { template: 'forecast-backtest' } });
    expect(p.globs).toEqual(expect.arrayContaining(['eval/**', 'data/**']));
    expect(p.entries.find((e) => e.glob === 'eval/**')).toMatchObject({ source: 'evaluator' });
    expect(effectiveProtected({ evaluator: { template: 'http-latency' } }).globs).toContain('bench/**');
  });

  it('an unknown template adds nothing (the config itself is invalid, parseConfig reports it)', () => {
    expect(effectiveProtected({ evaluator: { template: 'nope' } }).globs).not.toContain('bench/**');
  });

  it('protects the environment image and a custom infra module', () => {
    const p = effectiveProtected({ ...base, environment: { image: '.devcontainer/Dockerfile' }, infra: 'ops/tofu' });
    expect(p.globs).toEqual(expect.arrayContaining(['.devcontainer/Dockerfile', '.devcontainer/**', 'ops/tofu']));
    expect(findProtectedViolations(['ops/tofu/main.tf', '.devcontainer/devcontainer.json'], p.globs)).toHaveLength(2);
    expect(effectiveProtected({ ...base, environment: { image: 'Dockerfile.bench' } }).globs).toContain('Dockerfile.bench');
  });

  it("the user's protected: list only adds to the automatic set", () => {
    const p = effectiveProtected({ ...base, protected: ['tests/contract/**', 'ratchet.yaml', 'bench/**'] });
    expect(p.entries.filter((e) => e.source === 'user').map((e) => e.glob)).toEqual(['tests/contract/**', 'bench/**']);
    expect(p.globs.filter((g) => g === 'ratchet.yaml')).toHaveLength(1); // not duplicated
    expect(p.globs).toContain('**/pnpm-lock.yaml');
  });

  it('is deterministic and de-duplicated', () => {
    const input = { evaluator: { template: 'http-latency', run: 'node bench/http-latency.mjs' }, protected: ['bench/**'] };
    const a = effectiveProtected(input);
    expect(effectiveProtected(input)).toEqual(a);
    expect(new Set(a.globs).size).toBe(a.globs.length);
  });

  it('splits install files from the evaluator set (the bench pins lockfiles through the dependency strategy)', () => {
    const p = effectiveProtected({ ...base, protected: ['extra/**'] });
    expect(p.evaluatorGlobs).toEqual(['ratchet.yaml', 'extra/**']);
    expect(p.installGlobs).toHaveLength(INSTALL_FILES.length);
    expect([...p.evaluatorGlobs, ...p.installGlobs].sort()).toEqual([...p.globs].sort());
  });

  it('gives every entry a reason in plain words', () => {
    for (const e of effectiveProtected({ evaluator: { template: 'memory' }, protected: ['x'] }).entries) expect(e.reason.length).toBeGreaterThan(5);
  });
});

describe('required globs (what the evaluator ref must hold)', () => {
  it('are the evaluator files and the user list, not ratchet.yaml, the environment or lockfiles', () => {
    const p = effectiveProtected({ evaluator: { run: 'node bench/run.mjs' }, environment: { image: 'Dockerfile' }, protected: ['tests/**'] });
    expect(p.required).toEqual(['bench/**', 'tests/**']);
    expect(p.evaluatorGlobs).toEqual(['ratchet.yaml', 'bench/**', 'Dockerfile', 'tests/**']);
  });
  it('are empty when nothing marks the evaluator (the bench then refuses to run)', () => {
    expect(effectiveProtected({ evaluator: { run: 'sh ./score.sh' } }).required).toEqual([]);
  });
});
