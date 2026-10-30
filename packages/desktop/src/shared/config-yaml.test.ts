import { parseConfig } from '@ratchet/core';
import { describe, expect, it } from 'vitest';
import { buildConfigYaml, DEFAULT_FORM, parseGuardrailLine } from './config-yaml.js';

describe('buildConfigYaml', () => {
  it('the default form produces a config that parseConfig accepts', () => {
    const r = parseConfig(buildConfigYaml(DEFAULT_FORM));
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.config.objective).toEqual({ metric: 'http_req_duration.p95', direction: 'minimize', min_gain: 0.03 });
      expect(r.config.guardrails).toHaveLength(3);
      expect(r.config.guardrails[0]).toEqual({ kind: 'command', run: 'pnpm test:unit' });
      expect(r.config.guardrails[1]).toMatchObject({ kind: 'threshold', metric: 'error_rate', op: '<', value: 0.001 });
      expect(r.config.protected).toEqual(['bench/**', 'ratchet.yaml']);
      expect(r.config.agent.allowed_instance_types).toEqual(['c7i.2xlarge', 'c7i.xlarge', 'm7i.large']);
    }
  });

  it('omits confirm_trace when blank and includes it when set', () => {
    expect(buildConfigYaml(DEFAULT_FORM)).not.toContain('confirm_trace');
    expect(buildConfigYaml({ ...DEFAULT_FORM, confirmTrace: 's3://sealed/t.json' })).toContain('confirm_trace: s3://sealed/t.json');
  });

  it('keeps YAML-special characters safe (quoted, not injected)', () => {
    const y = buildConfigYaml({ ...DEFAULT_FORM, benchRun: 'echo "hi": # not a comment', guardrails: 'run: a: b' });
    const r = parseConfig(y);
    expect(r.ok).toBe(true);
    if (r.ok) {
      expect(r.config.bench.run).toBe('echo "hi": # not a comment');
      expect(r.config.guardrails[0]).toEqual({ kind: 'command', run: 'a: b' });
    }
  });

  it('lets the validator report malformed form values', () => {
    const r = parseConfig(buildConfigYaml({ ...DEFAULT_FORM, guardrails: 'error_rate < 0.001', minGainPct: 250, repeats: 0 }));
    expect(r.ok).toBe(false);
  });

  it('parseGuardrailLine splits on the first colon only', () => {
    expect(parseGuardrailLine('run: pnpm test')).toEqual({ run: 'pnpm test' });
    expect(parseGuardrailLine('peak_rss: < 1.2x')).toEqual({ peak_rss: '< 1.2x' });
    expect(parseGuardrailLine('nonsense')).toBe('nonsense');
  });
});
