import { describe, expect, it } from 'vitest';
import { parseConfig } from '../config/load.js';
import { effectiveProtected } from '../protected-set.js';
import { findProtectedViolations } from '../protected.js';
import { applyEnvironmentToConfigYaml, readEnvironmentScope } from './config-yaml.js';
import { environmentModuleOf, environmentSandboxId, hashEnvironmentFiles } from './environment.js';
import { draftPassed } from './types.js';

const CONFIG = `# my config
version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
evaluator:
  template: http-latency   # trusted evaluation
protected:
  - tests/**
`;

describe('environment.scope in ratchet.yaml', () => {
  it('is optional and validated', () => {
    expect(parseConfig(CONFIG).ok).toBe(true);
    const ok = parseConfig(applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' }));
    expect(ok.ok && ok.config.environment?.scope).toBe('full');
    const bad = parseConfig(CONFIG + 'environment:\n  scope: huge\n');
    expect(bad.ok).toBe(false);
    expect(!bad.ok && bad.errors[0]!.message).toMatch(/environment\.scope must be one of none, single, full/);
  });

  it('applyEnvironmentToConfigYaml sets only environment keys and keeps comments, order and the rest', () => {
    const out = applyEnvironmentToConfigYaml(CONFIG, { scope: 'single', machine: 'c7i.xlarge', image: 'infra/ratchet/Dockerfile' });
    expect(out).toContain('# my config');
    expect(out).toContain('# trusted evaluation');
    expect(out).toContain('tests/**');
    expect(readEnvironmentScope(out)).toBe('single');
    const parsed = parseConfig(out);
    expect(parsed.ok && parsed.config.environment).toMatchObject({ scope: 'single', machine: 'c7i.xlarge', image: 'infra/ratchet/Dockerfile' });
    expect(parsed.ok && parsed.config.evaluator.hardware).toBe('c7i.xlarge'); // environment.machine pins the evaluator host
    // changing the scope again keeps the other environment keys
    expect(applyEnvironmentToConfigYaml(out, { scope: 'none' })).toContain('machine: c7i.xlarge');
    expect(readEnvironmentScope('not: [valid')).toBeUndefined();
    expect(readEnvironmentScope(CONFIG)).toBeUndefined();
  });
});

describe('the approved environment is locked (automatic protected paths)', () => {
  const cfg = (yaml: string) => {
    const r = parseConfig(yaml);
    if (!r.ok) throw new Error(JSON.stringify(r.errors));
    return r.config;
  };

  it('protects infra/ratchet/** as soon as environment.scope is set, whatever the scope', () => {
    for (const scope of ['none', 'single', 'full'] as const) {
      const p = effectiveProtected(cfg(applyEnvironmentToConfigYaml(CONFIG, { scope })));
      expect(p.globs).toContain('infra/ratchet/**');
      expect(p.entries.find((e) => e.glob === 'infra/ratchet/**')).toMatchObject({ source: 'environment', reason: 'the approved environment' });
    }
    expect(effectiveProtected(cfg(CONFIG)).globs).not.toContain('infra/ratchet/**');
  });

  it('a candidate that touches the drafted environment or the config is blocked; plain code is not', () => {
    const p = effectiveProtected(cfg(applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' })));
    const v = findProtectedViolations(['src/app.ts', 'infra/ratchet/main.tf', 'infra/ratchet/scripts/api.sh', 'ratchet.yaml', 'infra/other/main.tf'], p.globs);
    expect(v.map((x) => x.path)).toEqual(['infra/ratchet/main.tf', 'infra/ratchet/scripts/api.sh', 'ratchet.yaml']);
  });

  it('an environment file is not "required" in the evaluator ref, so it never makes the bench refuse to run', () => {
    const p = effectiveProtected(cfg(applyEnvironmentToConfigYaml(CONFIG, { scope: 'full' })));
    expect(p.required).not.toContain('infra/ratchet/**');
    expect(p.evaluatorGlobs).toContain('infra/ratchet/**');
  });
});

describe('environmentModuleOf', () => {
  it('none / unset: the standard sandbox, no module', () => {
    expect(environmentModuleOf({})).toBeNull();
    expect(environmentModuleOf({ environment: { scope: 'none' } })).toBeNull();
  });
  it('full: the drafted module, required; single: the drafted module if there is one', () => {
    expect(environmentModuleOf({ environment: { scope: 'full' } })).toEqual({ kind: 'drafted', path: 'infra/ratchet', required: true });
    expect(environmentModuleOf({ environment: { scope: 'single' } })).toEqual({ kind: 'drafted', path: 'infra/ratchet', required: false });
  });
  it('infra: is the advanced override and wins', () => {
    expect(environmentModuleOf({ infra: './ops//sandbox', environment: { scope: 'full' } })).toEqual({ kind: 'custom', path: 'ops/sandbox', required: true });
  });
});

describe('hashEnvironmentFiles / environmentSandboxId', () => {
  const a = [{ path: 'main.tf', content: 'a' }, { path: 'x.sh', content: 'b' }];
  it('is stable, order independent and sensitive to paths and bytes', () => {
    expect(hashEnvironmentFiles(a)).toMatch(/^[0-9a-f]{10}$/);
    expect(hashEnvironmentFiles(a)).toBe(hashEnvironmentFiles([...a].reverse()));
    expect(hashEnvironmentFiles(a)).not.toBe(hashEnvironmentFiles([{ path: 'main.tf', content: 'a2' }, a[1]!]));
    expect(hashEnvironmentFiles(a)).not.toBe(hashEnvironmentFiles([{ path: 'main2.tf', content: 'a' }, a[1]!]));
    expect(hashEnvironmentFiles([{ path: 'ab', content: 'c' }])).not.toBe(hashEnvironmentFiles([{ path: 'a', content: 'bc' }]));
  });
  it('the sandbox id is a valid, region- and environment-specific id', () => {
    const id = environmentSandboxId('us-east-1', 'abc1234567');
    expect(id).toMatch(/^ratchet-env-[0-9a-f]{8}$/);
    expect(id.length).toBeLessThanOrEqual(32);
    expect(environmentSandboxId('eu-west-1', 'abc1234567')).not.toBe(id);
    expect(environmentSandboxId('us-east-1', 'abc1234568')).not.toBe(id);
    expect(environmentSandboxId('us-east-1', 'abc1234567')).toBe(id);
  });
});

describe('draftPassed', () => {
  it('needs checks and all of them ok', () => {
    expect(draftPassed({ checks: [] })).toBe(false);
    expect(draftPassed({ checks: [{ id: 'a', label: 'a', ok: true, detail: '' }] })).toBe(true);
    expect(draftPassed({ checks: [{ id: 'a', label: 'a', ok: true, detail: '' }, { id: 'b', label: 'b', ok: false, detail: '' }] })).toBe(false);
  });
});
