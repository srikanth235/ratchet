import { describe, expect, it } from 'vitest';
import { ANSWER_MARKER, MAIN_TF_SKELETON, buildReviseBrief, buildSetupBrief, formatAgentAnswer, parseAgentAnswer } from './brief.js';
import { validateEnvironment } from './validate.js';
import { applyEnvironmentToConfigYaml } from './config-yaml.js';

describe('agent answer', () => {
  it('round-trips and is found anywhere in the final message', () => {
    const a = { scope: 'full' as const, reason: 'p95 depends on database queries, so a single machine would not measure it.' };
    expect(parseAgentAnswer(formatAgentAnswer(a))).toEqual(a);
    expect(parseAgentAnswer(`Done! I wrote the files.\n\n${formatAgentAnswer(a)}\nBye`)).toEqual(a);
    expect(parseAgentAnswer(`${ANSWER_MARKER}\n\`\`\`json\n{"scope":"none","reason":"x"}\n\`\`\``)).toEqual({ scope: 'none', reason: 'x' });
  });

  it('the last marker wins and braces inside the reason do not confuse it', () => {
    const text = `${formatAgentAnswer({ scope: 'single', reason: 'first' })}\n${ANSWER_MARKER} {"scope":"full","reason":"a } brace and a \\"quote\\""}`;
    expect(parseAgentAnswer(text)).toEqual({ scope: 'full', reason: 'a } brace and a "quote"' });
  });

  it('is null for a missing marker, bad JSON or an unknown scope', () => {
    expect(parseAgentAnswer('no answer here')).toBeNull();
    expect(parseAgentAnswer(`${ANSWER_MARKER} {not json}`)).toBeNull();
    expect(parseAgentAnswer(`${ANSWER_MARKER} {"scope":"huge","reason":"x"}`)).toBeNull();
    expect(parseAgentAnswer(`${ANSWER_MARKER} {"scope":"full"`)).toBeNull();
    expect(parseAgentAnswer(`${ANSWER_MARKER} {"scope":"full"}`)).toEqual({ scope: 'full', reason: '' });
  });
});

describe('setup brief', () => {
  const brief = buildSetupBrief({ goal: 'Make checkout p95 faster', hardware: 'c7i.2xlarge', timeLimitSeconds: 7200 });

  it('carries the goal, the three scopes, the rules the validator enforces and the answer line', () => {
    expect(brief).toContain('Make checkout p95 faster');
    for (const s of ['`none`', '`single`', '`full`']) expect(brief).toContain(s);
    expect(brief).toContain('infra/ratchet/');
    expect(brief).toContain('./ratchet-sandbox');
    expect(brief).toContain('NO internet');
    expect(brief).toContain('2.0 h');
    expect(brief).toContain(ANSWER_MARKER);
    expect(brief).toMatch(/Do not edit source code/);
  });

  it('lists the allowed machine keys and the priced instance types', () => {
    expect(brief).toContain('role, instance_type, label, detail, user_data, root_volume_gb, data_volume_gb');
    expect(brief).toContain('c7i.2xlarge');
    expect(brief).toContain('r7g.large');
  });

  it('the skeleton it shows is itself a draft that passes the validator', () => {
    const cfg = 'version: 0\nobjective:\n  metric: m\n  direction: minimize\nevaluator:\n  template: http-latency\n';
    const r = validateEnvironment({
      scope: 'full',
      originalConfigYaml: cfg,
      files: [
        { path: 'ratchet.yaml', content: applyEnvironmentToConfigYaml(cfg, { scope: 'full' }) },
        { path: 'infra/ratchet/main.tf', content: MAIN_TF_SKELETON },
        { path: 'infra/ratchet/scripts/api.sh', content: '#!/bin/bash\n' },
        { path: 'infra/ratchet/scripts/load.sh', content: '#!/bin/bash\n' },
      ],
    });
    expect(r.problems).toEqual([]);
  });

  it('the revise brief repeats the change and the answer line', () => {
    const b = buildReviseBrief('use RDS instead of a Postgres container');
    expect(b).toContain('use RDS instead of a Postgres container');
    expect(b).toContain(ANSWER_MARKER);
  });
});
