import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  evaluateCommandResult,
  evaluateThreshold,
  formatConfigError,
  loadConfig,
  parseConfig,
  parseThreshold,
  type ThresholdGuardrail,
} from './index.js';

const VALID = `
version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: k6 run bench/k6/search.js
  repeats: 5
  hardware: c7i.2xlarge
  confirm_trace: s3://sealed/search-trace.jsonl
guardrails:
  - run: pytest -q
  - run: openapi-diff --base main
  - error_rate: "< 0.001"
  - peak_rss: "< 1.2x"
protected:
  - bench/**
  - ratchet.yaml
  - tests/contract/**
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge, c7i.xlarge, m7i.large]
`;

function errorsOf(text: string) {
  const r = parseConfig(text);
  if (r.ok) throw new Error('expected failure');
  return r.errors;
}

/** Replace a line in VALID. */
const mutate = (from: string, to: string) => {
  expect(VALID).toContain(from);
  return VALID.replace(from, to);
};

describe('parseConfig', () => {
  it('parses the §4 example', () => {
    const r = parseConfig(VALID);
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.config.objective).toEqual({ metric: 'http_req_duration.p95', direction: 'minimize', min_gain: 0.03 });
    expect(r.config.bench.repeats).toBe(5);
    expect(r.config.bench.confirm_trace).toBe('s3://sealed/search-trace.jsonl');
    expect(r.config.protected).toEqual(['bench/**', 'ratchet.yaml', 'tests/contract/**']);
    expect(r.config.agent.allowed_instance_types).toHaveLength(3);
    expect(r.config.guardrails).toEqual([
      { kind: 'command', run: 'pytest -q' },
      { kind: 'command', run: 'openapi-diff --base main' },
      { kind: 'threshold', metric: 'error_rate', op: '<', value: 0.001, relative: false, raw: '< 0.001' },
      { kind: 'threshold', metric: 'peak_rss', op: '<', value: 1.2, relative: true, raw: '< 1.2x' },
    ]);
  });

  it('accepts maximize and omits optional confirm_trace, guardrails, protected', () => {
    const text = `
version: 0
objective: { metric: rps, direction: maximize, min_gain: 0.05 }
bench: { ref: r, run: ./bench.sh, repeats: 3, hardware: c7i.xlarge }
agent: { instructions: RATCHET.md, allowed_instance_types: [c7i.xlarge] }
`;
    const r = parseConfig(text);
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.config.objective.direction).toBe('maximize');
    expect(r.config.bench.confirm_trace).toBeUndefined();
    expect(r.config.guardrails).toEqual([]);
    expect(r.config.protected).toEqual([]);
  });

  it('reports invalid YAML syntax with a line number', () => {
    const errs = errorsOf('version: 0\nobjective: [unclosed\n');
    expect(errs[0]!.message).toMatch(/^invalid YAML \(line \d+, column \d+\)/);
  });

  it('reports empty and non-mapping documents', () => {
    expect(errorsOf('')[0]!.message).toMatch(/empty/);
    expect(errorsOf('- a\n- b')[0]!.message).toMatch(/mapping/);
    expect(errorsOf('just a string')[0]!.message).toMatch(/mapping/);
  });

  it('reports missing required sections with their paths', () => {
    const errs = errorsOf('version: 0\n');
    const paths = errs.map((e) => e.path).sort();
    expect(paths).toEqual(['agent', 'evaluator', 'objective']);
    expect(formatConfigError(errs[0]!)).toMatch(/^(agent|evaluator|objective): /);
  });

  it('reports a wrong version', () => {
    const errs = errorsOf(mutate('version: 0', 'version: 1'));
    expect(errs).toHaveLength(1);
    expect(errs[0]!.path).toBe('version');
    expect(errs[0]!.message).toMatch(/must be 0/);
  });

  it('reports a bad direction at objective.direction', () => {
    const errs = errorsOf(mutate('direction: minimize', 'direction: lower'));
    expect(errs).toEqual([{ path: 'objective.direction', message: 'direction must be "minimize" or "maximize"' }]);
  });

  it('validates min_gain range and type', () => {
    expect(errorsOf(mutate('min_gain: 0.03', 'min_gain: 3'))[0]!.path).toBe('objective.min_gain');
    expect(errorsOf(mutate('min_gain: 0.03', 'min_gain: -0.1'))[0]!.message).toMatch(/>= 0/);
    expect(errorsOf(mutate('min_gain: 0.03', 'min_gain: 3%'))[0]!.message).toMatch(/must be a number/);
    expect(parseConfig(mutate('min_gain: 0.03', 'min_gain: 0')).ok).toBe(true);
  });

  it('validates repeats', () => {
    expect(errorsOf(mutate('repeats: 5', 'repeats: 0'))[0]!.path).toBe('evaluator.repeats');
    expect(errorsOf(mutate('repeats: 5', 'repeats: 2.5'))[0]!.message).toMatch(/integer/);
  });

  it('rejects unknown keys (typos) with their location', () => {
    const errs = errorsOf(mutate('  hardware: c7i.2xlarge', '  hardware: c7i.2xlarge\n  hardwre: x'));
    expect(errs).toEqual([{ path: 'evaluator', message: 'unknown key "hardwre"' }]);
    expect(errorsOf(VALID + 'extra: 1\n')[0]).toEqual({ path: '', message: 'unknown key "extra"' });
  });

  it('reports bad guardrail thresholds with the index and metric in the path', () => {
    const errs = errorsOf(mutate('error_rate: "< 0.001"', 'error_rate: "about 0.001"'));
    expect(errs).toHaveLength(1);
    expect(errs[0]!.path).toBe('guardrails[2].error_rate');
    expect(errs[0]!.message).toMatch(/invalid threshold "about 0.001"/);
  });

  it('rejects guardrails with several keys, no keys, or non-string values', () => {
    expect(errorsOf(mutate('- run: pytest -q', '- run: pytest -q\n    error_rate: "< 1"'))[0]!.message).toMatch(
      /exactly one key/,
    );
    expect(errorsOf(mutate('- run: pytest -q', '- {}'))[0]!.message).toMatch(/exactly one key.*found 0/);
    const numeric = errorsOf(mutate('error_rate: "< 0.001"', 'error_rate: 0.001'));
    expect(numeric[0]!.path).toBe('guardrails[2].error_rate');
    expect(numeric[0]!.message).toMatch(/must be strings/);
    expect(errorsOf(mutate('- run: pytest -q', '- pytest'))[0]!.path).toBe('guardrails[0]');
  });

  it('rejects empty run guardrail', () => {
    const errs = errorsOf(mutate('- run: pytest -q', '- run: ""'));
    expect(errs[0]).toEqual({ path: 'guardrails[0].run', message: '`run` must be a non-empty command' });
  });

  it('requires at least one instance type and string globs', () => {
    expect(errorsOf(mutate('[c7i.2xlarge, c7i.xlarge, m7i.large]', '[]'))[0]!.path).toBe(
      'agent.allowed_instance_types',
    );
    expect(errorsOf(mutate('  - ratchet.yaml', '  - 5'))[0]!.path).toBe('protected[1]');
  });

  it('collects multiple errors at once', () => {
    const errs = errorsOf(
      mutate('direction: minimize', 'direction: sideways').replace('repeats: 5', 'repeats: 0'),
    );
    expect(errs.map((e) => e.path).sort()).toEqual(['evaluator.repeats', 'objective.direction']);
  });
});

describe('loadConfig', () => {
  it('loads from disk', () => {
    const dir = mkdtempSync(join(tmpdir(), 'ratchet-cfg-'));
    const p = join(dir, 'ratchet.yaml');
    writeFileSync(p, VALID);
    const r = loadConfig(p);
    expect(r.ok).toBe(true);
  });

  it('reports a missing file as an error', () => {
    const r = loadConfig('/nonexistent/dir/ratchet.yaml');
    expect(r).toEqual({ ok: false, errors: [{ path: '', message: 'cannot find /nonexistent/dir/ratchet.yaml' }] });
  });
});

describe('parseThreshold', () => {
  const ok = (s: string) => {
    const r = parseThreshold('m', s);
    if (typeof r === 'string') throw new Error(r);
    return r;
  };
  it.each([
    ['< 0.001', '<', 0.001, false],
    ['<=5', '<=', 5, false],
    ['>= 0.99', '>=', 0.99, false],
    ['> 10', '>', 10, false],
    ['== 0', '==', 0, false],
    ['< 1.2x', '<', 1.2, true],
    ['  <  2 x ', '<', 2, true],
    ['< 1e-3', '<', 0.001, false],
    ['< .5', '<', 0.5, false],
    ['> -1', '>', -1, false],
  ])('parses %j', (raw, op, value, relative) => {
    expect(ok(raw)).toMatchObject({ metric: 'm', op, value, relative });
  });
  it.each(['', '0.001', '< ', '<< 1', '< abc', '< 1.2y', '< 1 2', '!= 3', '< 0x'])('rejects %j', (raw) => {
    expect(typeof parseThreshold('m', raw)).toBe('string');
  });
});

describe('guardrail evaluation', () => {
  const g = (raw: string) => parseThreshold('m', raw) as ThresholdGuardrail;
  it('absolute thresholds', () => {
    expect(evaluateThreshold(g('< 0.001'), 0.0005).passed).toBe(true);
    expect(evaluateThreshold(g('< 0.001'), 0.001).passed).toBe(false);
    expect(evaluateThreshold(g('<= 0.001'), 0.001).passed).toBe(true);
    expect(evaluateThreshold(g('>= 0.99'), 0.98).passed).toBe(false);
  });
  it('relative thresholds need a baseline', () => {
    expect(evaluateThreshold(g('< 1.2x'), 110, 100).passed).toBe(true);
    expect(evaluateThreshold(g('< 1.2x'), 121, 100).passed).toBe(false);
    const r = evaluateThreshold(g('< 1.2x'), 110);
    expect(r.passed).toBe(false);
    expect(r.detail).toMatch(/baseline/);
  });
  it('non-finite observations fail', () => {
    expect(evaluateThreshold(g('< 1'), NaN).passed).toBe(false);
  });
  it('command results', () => {
    expect(evaluateCommandResult({ run: 'pytest -q' }, 0)).toEqual({ name: 'pytest -q', passed: true, detail: undefined });
    expect(evaluateCommandResult({ run: 'pytest -q' }, 1).passed).toBe(false);
  });
});

describe('generic evaluator + value', () => {
  const generic = `
version: 0
pack: forecasting
objective:
  name: Weekly sales forecast
  metric: wape
  direction: minimize
  min_gain: 0.02
  value: { usd_per_unit: 250000, period: year, unit: "WAPE point" }
evaluator:
  kind: backtest
  ref: ratchet/eval
  run: python -m eval.backtest
  repeats: 3
  hardware: m7i.xlarge
  holdout: s3://sealed/last-8-weeks.parquet
agent: { instructions: RATCHET.md, allowed_instance_types: [m7i.xlarge] }
`;
  it('parses `evaluator`, holdout and value, and aliases it as `bench`', () => {
    const r = parseConfig(generic);
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.config.evaluator.kind).toBe('backtest');
    expect(r.config.evaluator.holdout).toBe('s3://sealed/last-8-weeks.parquet');
    expect(r.config.bench.confirm_trace).toBe('s3://sealed/last-8-weeks.parquet');
    expect(r.config.bench).toBe(r.config.evaluator);
    expect(r.config.objective.value).toEqual({ usd_per_unit: 250000, period: 'year', basis: 'estimated', unit: 'WAPE point' });
    expect(r.config.pack).toBe('forecasting');
  });
  it('defaults evaluator.kind to benchmark for legacy `bench:` configs', () => {
    const r = parseConfig(generic.replace('evaluator:', 'bench:').replace('  kind: backtest\n', ''));
    expect(r.ok && r.config.evaluator.kind).toBe('benchmark');
  });
  it('rejects a non-positive value', () => {
    const r = parseConfig(generic.replace('usd_per_unit: 250000', 'usd_per_unit: 0'));
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.errors[0]!.path).toBe('objective.value.usd_per_unit');
  });
});

describe('evaluator.deps', () => {
  const base = `
version: 0
objective: { metric: p95, direction: minimize, min_gain: 0.03 }
evaluator:
  ref: ratchet/bench
  run: node bench.mjs
  repeats: 3
  hardware: c7i.xlarge
agent: { instructions: RATCHET.md, allowed_instance_types: [c7i.xlarge] }
`;
  const withDeps = (value: string) => base.replace('  hardware: c7i.xlarge', `  hardware: c7i.xlarge\n  deps: ${value}`);
  it('defaults to auto (detect the package manager from the lockfile at evaluator.ref)', () => {
    const r = parseConfig(base);
    expect(r.ok && r.config.evaluator.deps).toBe('auto');
    expect(r.ok && r.config.bench.deps).toBe('auto');
  });
  it.each(['auto', 'pnpm', 'npm', 'none'])('accepts %s, also under the legacy bench: key', (mode) => {
    const r = parseConfig(withDeps(mode));
    expect(r.ok && r.config.evaluator.deps).toBe(mode);
    const legacy = parseConfig(withDeps(mode).replace('evaluator:', 'bench:'));
    expect(legacy.ok && legacy.config.bench.deps).toBe(mode);
  });
  it('rejects other package managers with the list of valid ones', () => {
    const r = parseConfig(withDeps('yarn'));
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.errors[0]!.path).toBe('evaluator.deps');
      expect(r.errors[0]!.message).toContain('auto, pnpm, npm, none');
    }
  });
  it('rejects a boolean (deps: false is not a way to say none)', () => {
    expect(parseConfig(withDeps('false')).ok).toBe(false);
  });
});
