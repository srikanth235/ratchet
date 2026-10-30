import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parseConfig } from './load.js';

const AGENT = `agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.large]
`;

const MINIMAL = `version: 0
objective: { metric: http_req_duration.p95, direction: minimize }
evaluator:
  template: http-latency
${AGENT}`;

function ok(text: string) {
  const r = parseConfig(text);
  if (!r.ok) throw new Error(r.errors.map((e) => `${e.path}: ${e.message}`).join('; '));
  return r.config;
}
function errors(text: string) {
  const r = parseConfig(text);
  if (r.ok) throw new Error('expected failure');
  return r.errors;
}

describe('ratchet.yaml, simplified model', () => {
  it('a template, an objective and an agent brief are enough', () => {
    const c = ok(MINIMAL);
    expect(c.objective.min_gain).toBeUndefined();
    expect(c.evaluator.repeats).toBeUndefined();
    expect(c.evaluator).toMatchObject({
      template: 'http-latency',
      run: 'node bench/http-latency.mjs',
      kind: 'benchmark',
      ref: 'ratchet/bench',
      hardware: 'c7i.2xlarge',
      deps: 'auto',
    });
    expect(c.guardrails).toEqual([{ kind: 'threshold', metric: 'error_rate', op: '<', value: 0.001, relative: false, raw: '< 0.001' }]);
    expect(c.bench).toBe(c.evaluator);
  });

  it('explicit values beat the template and the environment', () => {
    const c = ok(`${MINIMAL}guardrails:\n  - run: npm test\nenvironment: { machine: m7i.large }\n`.replace('template: http-latency', 'template: http-latency\n  run: node my/bench.js\n  hardware: c7i.4xlarge\n  kind: simulation'));
    expect(c.evaluator).toMatchObject({ run: 'node my/bench.js', hardware: 'c7i.4xlarge', kind: 'simulation' });
    expect(c.guardrails).toEqual([{ kind: 'command', run: 'npm test' }]);
  });

  it('hardware falls back to environment.machine, then the template, then c7i.large', () => {
    expect(ok(`${MINIMAL}environment: { machine: m7i.xlarge }\n`).evaluator.hardware).toBe('m7i.xlarge');
    expect(ok(MINIMAL.replace('template: http-latency', 'run: ./b.sh')).evaluator.hardware).toBe('c7i.large');
    expect(ok(MINIMAL.replace('template: http-latency', 'template: build-size')).evaluator.hardware).toBe('c7i.xlarge');
  });

  it('repeats, min_gain and noise_factor are optional but validated when present', () => {
    const c = ok(MINIMAL.replace('direction: minimize', 'direction: minimize, min_gain: 0.05').replace('template: http-latency', 'template: http-latency\n  repeats: 7\n  noise_factor: 3'));
    expect(c.objective.min_gain).toBe(0.05);
    expect(c.evaluator).toMatchObject({ repeats: 7, noise_factor: 3 });
    expect(errors(MINIMAL.replace('template: http-latency', 'template: http-latency\n  repeats: 0'))[0]!.path).toBe('evaluator.repeats');
    expect(errors(MINIMAL.replace('template: http-latency', 'template: http-latency\n  noise_factor: -1'))[0]!.path).toBe('evaluator.noise_factor');
    expect(errors(MINIMAL.replace('direction: minimize', 'direction: minimize, min_gain: 3'))[0]!.path).toBe('objective.min_gain');
  });

  it('needs either run or template', () => {
    const e = errors(MINIMAL.replace('  template: http-latency\n', '  hardware: c7i.large\n'));
    expect(e).toHaveLength(1);
    expect(e[0]!.path).toBe('evaluator.run');
    expect(e[0]!.message).toMatch(/run.*template/);
  });

  it('rejects an unknown template and a demo, listing the valid ids', () => {
    const e = errors(MINIMAL.replace('http-latency', 'nope'));
    expect(e[0]!.path).toBe('evaluator.template');
    expect(e[0]!.message).toMatch(/unknown evaluator template "nope".*http-latency.*llm-eval/);
    expect(errors(MINIMAL.replace('http-latency', 'demo-search'))[0]!.message).toMatch(/unknown evaluator template/);
    expect(errors(MINIMAL.replace('http-latency', 'Bad_Id'))[0]!.path).toBe('evaluator.template');
  });

  it('the bench: alias works with a template too', () => {
    expect(ok(MINIMAL.replace('evaluator:', 'bench:')).evaluator.run).toBe('node bench/http-latency.mjs');
  });

  describe('environment', () => {
    it('accepts image, machine, region and network', () => {
      const c = ok(`${MINIMAL}environment:
  image: .devcontainer/Dockerfile
  machine: c7i.2xlarge
  region: eu-west-1
  network: [registry.npmjs.org, "*.pythonhosted.org", files.pythonhosted.org]
`);
      expect(c.environment).toEqual({
        image: '.devcontainer/Dockerfile',
        machine: 'c7i.2xlarge',
        region: 'eu-west-1',
        network: ['registry.npmjs.org', '*.pythonhosted.org', 'files.pythonhosted.org'],
      });
    });
    it('all four are optional, an empty block is fine, unknown keys are not', () => {
      expect(ok(`${MINIMAL}environment: {}\n`).environment).toEqual({});
      expect(errors(`${MINIMAL}environment: { gpu: true }\n`)[0]).toEqual({ path: 'environment', message: 'unknown key "gpu"' });
    });
    it.each([
      ['image: /etc/passwd', 'environment.image', /relative/],
      ['image: ../outside/Dockerfile', 'environment.image', /no "\.\."/],
      ['image: ""', 'environment.image', /must not be empty/],
      ['machine: big', 'environment.machine', /instance type/],
      ['machine: C7I.Large', 'environment.machine', /instance type/],
      ['region: useast1', 'environment.region', /AWS region/],
      ['region: us-east', 'environment.region', /AWS region/],
      ['network: ["https://registry.npmjs.org"]', 'environment.network[0]', /host names/],
      ['network: ["registry.npmjs.org:443"]', 'environment.network[0]', /host names/],
      ['network: ["*"]', 'environment.network[0]', /host names/],
      ['network: ["localhost"]', 'environment.network[0]', /host names/],
      ['network: registry.npmjs.org', 'environment.network', /list of host names/],
    ])('rejects %s', (line, path, message) => {
      const e = errors(`${MINIMAL}environment: { ${line} }\n`);
      expect(e[0]!.path).toBe(path);
      expect(e[0]!.message).toMatch(message);
    });
  });

  describe('infra (advanced override, read from the protected ref only)', () => {
    it('is a repo path, off by default', () => {
      expect(ok(MINIMAL).infra).toBeUndefined();
      expect(ok(`${MINIMAL}infra: ops/sandbox\n`).infra).toBe('ops/sandbox');
    });
    it('cannot point outside the repository', () => {
      expect(errors(`${MINIMAL}infra: ../sandbox\n`)[0]!.path).toBe('infra');
      expect(errors(`${MINIMAL}infra: /abs/path\n`)[0]!.path).toBe('infra');
    });
  });

  it('needs only objective and evaluator; the agent block defaults (brief RATCHET.md, the bench machine)', () => {
    expect(errors('version: 0\n').map((e) => e.path).sort()).toEqual(['evaluator', 'objective']);
    const r = parseConfig('version: 0\nobjective: { metric: http_req_duration.p95, direction: minimize }\nevaluator: { template: http-latency }\nenvironment: { machine: c7i.xlarge }\n');
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.config.agent).toEqual({ instructions: 'RATCHET.md', allowed_instance_types: ['c7i.xlarge'] });
  });
});

describe('existing configs stay valid', () => {
  const root = join(__dirname, '..', '..', '..', '..');
  const files = ['examples/demo-search/ratchet.yaml', 'examples/demo-cli/ratchet.yaml', 'examples/demo-forecast/ratchet.yaml'];
  it.each(files)('%s', (file) => {
    const c = ok(readFileSync(join(root, file), 'utf8'));
    expect(c.evaluator.run).toBeTruthy();
    expect(c.objective.min_gain).toBeGreaterThan(0);
  });
  it('the example directories are the ones listed', () => {
    expect(readdirSync(join(root, 'examples')).filter((d) => d.startsWith('demo-')).sort()).toEqual(['demo-cli', 'demo-forecast', 'demo-search']);
  });
});
