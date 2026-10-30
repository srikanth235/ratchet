import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseConfig } from '@ratchet/core';
import { describe, expect, it } from 'vitest';
import { buildConfigYaml, DEFAULT_FORM, formFromYaml, parseGuardrailLine, patchConfigYaml } from './config-yaml.js';

const DEMO_YAML = readFileSync(resolve(__dirname, '../../../../examples/demo-search/ratchet.yaml'), 'utf8');
const FORECAST_YAML = readFileSync(resolve(__dirname, '../../../../examples/demo-forecast/ratchet.yaml'), 'utf8');

describe('buildConfigYaml', () => {
  it('the default form produces a config that parseConfig accepts, under `evaluator:`', () => {
    const y = buildConfigYaml(DEFAULT_FORM);
    expect(y).toMatch(/^evaluator:\n {2}kind: benchmark/m);
    expect(y).not.toMatch(/^bench:/m);
    const r = parseConfig(y);
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.config.objective).toEqual({ metric: 'http_req_duration.p95', direction: 'minimize', min_gain: 0.03 });
      expect(r.config.evaluator).toMatchObject({ kind: 'benchmark', ref: 'ratchet/bench', run: 'k6 run bench/k6/search.js', repeats: 5 });
      expect(r.config.guardrails).toHaveLength(3);
      expect(r.config.guardrails[0]).toEqual({ kind: 'command', run: 'pnpm test:unit' });
      expect(r.config.guardrails[1]).toMatchObject({ kind: 'threshold', metric: 'error_rate', op: '<', value: 0.001 });
      expect(r.config.protected).toEqual(['bench/**', 'ratchet.yaml']);
      expect(r.config.agent.allowed_instance_types).toEqual(['c7i.2xlarge', 'c7i.xlarge', 'm7i.large']);
    }
  });

  it('writes holdout and objective.value only when set', () => {
    expect(buildConfigYaml(DEFAULT_FORM)).not.toMatch(/holdout|value:/);
    const r = parseConfig(buildConfigYaml({ ...DEFAULT_FORM, holdout: 's3://sealed/t.json', valueUsdPerUnit: 1200, valueUnit: 'ms' }));
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.config.evaluator.holdout).toBe('s3://sealed/t.json');
      expect(r.config.objective.value).toEqual({ usd_per_unit: 1200, period: 'year', basis: 'estimated', unit: 'ms' });
    }
  });

  it('keeps YAML-special characters safe (quoted, not injected)', () => {
    const y = buildConfigYaml({ ...DEFAULT_FORM, evaluatorRun: 'echo "hi": # not a comment', guardrails: ['run: a: b'] });
    const r = parseConfig(y);
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.config.evaluator.run).toBe('echo "hi": # not a comment');
      expect(r.config.guardrails[0]).toEqual({ kind: 'command', run: 'a: b' });
    }
  });

  it('lets the validator report malformed form values', () => {
    const r = parseConfig(buildConfigYaml({ ...DEFAULT_FORM, guardrails: ['error_rate < 0.001'], minGainPct: 250, repeats: 0 }));
    expect(r.ok).toBe(false);
  });

  it('parseGuardrailLine splits on the first colon only', () => {
    expect(parseGuardrailLine('run: pnpm test')).toEqual({ run: 'pnpm test' });
    expect(parseGuardrailLine('peak_rss: < 1.2x')).toEqual({ peak_rss: '< 1.2x' });
    expect(parseGuardrailLine('nonsense')).toBe('nonsense');
  });
});

describe('formFromYaml / patchConfigYaml (the form is a two-way view of the text)', () => {
  it('reads back what buildConfigYaml wrote', () => {
    const f = { ...DEFAULT_FORM, holdout: 's3://x', valueUsdPerUnit: 20000, valuePeriod: 'month' as const, valueBasis: 'measured' as const, valueUnit: 'WAPE point' };
    expect(formFromYaml(buildConfigYaml(f))).toEqual(f);
  });

  it('reads the shipped demo configs: evaluator kind, holdout, value, guardrails', () => {
    const s = formFromYaml(DEMO_YAML)!;
    expect(s).toMatchObject({ metric: 'http_req_duration.p95', evaluatorKind: 'benchmark', holdout: 's3://sealed/demo-search-trace.json', valueUsdPerUnit: 1200, valueUnit: 'ms' });
    expect(s.guardrails).toEqual(['run: pnpm test:unit', 'run: pnpm test:contract', 'error_rate: < 0.001', 'peak_rss: < 1.2x']);
    expect(formFromYaml(FORECAST_YAML)).toMatchObject({ metric: 'wape', evaluatorKind: 'backtest', valueUsdPerUnit: 20000, valueUnit: 'WAPE point' });
  });

  it('patches only the changed keys and keeps comments and unknown keys', () => {
    const y = patchConfigYaml(DEMO_YAML, { evaluatorKind: 'simulation', minGainPct: 5, guardrails: ['run: pnpm test:unit'] });
    expect(y).toContain('# domain pack: code performance');
    expect(y).toContain('pack: perf');
    expect(y).toMatch(/allowed_instance_types: \[c7i\.2xlarge/);
    const r = parseConfig(y);
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.config.evaluator.kind).toBe('simulation');
      expect(r.config.objective.min_gain).toBe(0.05);
      expect(r.config.guardrails).toEqual([{ kind: 'command', run: 'pnpm test:unit' }]);
      expect(r.config.objective.value?.usd_per_unit).toBe(1200);
    }
  });

  it('adds and removes objective.value; blank holdout drops the key (and the legacy confirm_trace)', () => {
    const base = buildConfigYaml(DEFAULT_FORM);
    const withValue = patchConfigYaml(base, { valueUsdPerUnit: 900 });
    expect(formFromYaml(withValue)).toMatchObject({ valueUsdPerUnit: 900, valuePeriod: 'year', valueBasis: 'estimated' });
    expect(parseConfig(withValue).ok).toBe(true);
    expect(patchConfigYaml(withValue, { valueUsdPerUnit: null })).not.toContain('value:');
    expect(patchConfigYaml(DEMO_YAML, { holdout: '' })).not.toMatch(/holdout|confirm_trace/);
  });

  it('writes to the legacy `bench:` key when that is what the text uses', () => {
    const legacy = DEMO_YAML.replace('evaluator:', 'bench:');
    const y = patchConfigYaml(legacy, { evaluatorRun: 'node dist/bench/local.js' });
    expect(y).not.toMatch(/^evaluator:/m);
    expect(formFromYaml(y)!.evaluatorRun).toBe('node dist/bench/local.js');
  });

  it('leaves text that is not a YAML mapping alone, and formFromYaml says so', () => {
    expect(formFromYaml('objective: [unclosed')).toBeNull();
    expect(patchConfigYaml('objective: [unclosed', { metric: 'x' })).toBe('objective: [unclosed');
    expect(formFromYaml('- a list')).toBeNull();
  });
});
