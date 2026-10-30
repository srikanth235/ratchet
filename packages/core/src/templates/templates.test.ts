import { execFile } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { promisify } from 'node:util';
import { afterAll, describe, expect, it } from 'vitest';
import { extractLastJsonObject, lookupPath } from '../bench/output.js';
import { parseConfig } from '../config/load.js';
import { effectiveProtected } from '../protected-set.js';
import { findProtectedViolations } from '../protected.js';
// @ts-expect-error plain ES module script without types
import { generate } from '../../scripts/gen-templates.mjs';
import { TEMPLATE_FILES } from './files.generated.js';
import { buildTemplateProposal } from './propose.js';
import { evaluatorTemplateIds, getTemplate, listTemplates, resolveTemplateId } from './registry.js';
import { renderTemplate, templateFilePaths, type RenderedTemplate } from './render.js';

const run = promisify(execFile);
const dirs: string[] = [];
afterAll(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

function write(rendered: RenderedTemplate): string {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-tpl-'));
  dirs.push(dir);
  for (const f of rendered.files) {
    mkdirSync(dirname(join(dir, f.path)), { recursive: true });
    writeFileSync(join(dir, f.path), f.content);
  }
  return dir;
}

async function runEvaluator(dir: string, command: string, env: Record<string, string> = {}): Promise<Record<string, unknown>> {
  const [bin, ...args] = command.split(' ');
  const { stdout } = await run(bin === 'node' ? process.execPath : bin!, args, {
    cwd: dir,
    env: { ...process.env, ...env },
    timeout: 60_000,
  });
  const json = extractLastJsonObject(stdout);
  if (!json) throw new Error(`evaluator printed no JSON object:\n${stdout}`);
  return json;
}

const TEMPLATE_IDS = ['http-latency', 'cli-cold-start', 'test-suite-time', 'build-size', 'memory', 'forecast-backtest', 'llm-eval'];

describe('registry', () => {
  it('has the seven templates of the spec table, then the three demos', () => {
    expect(listTemplates({ kind: 'template' }).map((t) => t.id)).toEqual(TEMPLATE_IDS);
    expect(listTemplates({ kind: 'demo' }).map((t) => t.id)).toEqual(['demo-search', 'demo-cli', 'demo-forecast']);
    expect(listTemplates().map((t) => t.id)).toEqual([...TEMPLATE_IDS, 'demo-search', 'demo-cli', 'demo-forecast']);
    expect(evaluatorTemplateIds()).toEqual(TEMPLATE_IDS);
  });

  it('carries the metadata the gallery shows', () => {
    const spec: Record<string, [string, string | null]> = {
      'http-latency': ['c7i.2xlarge', 'hidden request mix'],
      'cli-cold-start': ['c7i.large', 'hidden invocations'],
      'test-suite-time': ['c7i.2xlarge', null],
      'build-size': ['c7i.xlarge', null],
      memory: ['c7i.xlarge', 'hidden workload'],
      'forecast-backtest': ['c7i.xlarge', 'most recent periods'],
      'llm-eval': ['c7i.large', 'hidden eval set'],
    };
    for (const t of listTemplates({ kind: 'template' })) {
      expect([t.machine, t.holdout]).toEqual(spec[t.id]);
      expect(t.title && t.description && t.measures).toBeTruthy();
      expect(['perf', 'forecasting', 'llm']).toContain(t.category);
      expect(t.files[0]).toBe('ratchet.yaml');
      expect(t.files).toContain('RATCHET.md');
    }
  });

  it('demos are metadata only and point at the examples', () => {
    for (const t of listTemplates({ kind: 'demo' })) {
      expect(t.files).toEqual([]);
      expect(existsSync(join(__dirname, '..', '..', '..', '..', t.example!, 'ratchet.yaml'))).toBe(true);
      expect(() => renderTemplate(t.id)).toThrow(/demo/);
    }
  });

  it('filters by category and resolves the legacy pack names', () => {
    expect(listTemplates({ category: 'llm' }).map((t) => t.id)).toEqual(['llm-eval']);
    expect(listTemplates({ category: 'forecasting' }).map((t) => t.id)).toEqual(['forecast-backtest', 'demo-forecast']);
    expect(resolveTemplateId('perf')).toBe('cli-cold-start');
    expect(resolveTemplateId('forecasting')).toBe('forecast-backtest');
    expect(resolveTemplateId('http-latency')).toBe('http-latency');
    expect(getTemplate('perf')?.id).toBe('cli-cold-start');
    expect(getTemplate('nope')).toBeUndefined();
  });

  it('the list is a copy: callers cannot corrupt the registry', () => {
    const a = listTemplates();
    a[0]!.title = 'x';
    a[0]!.objective.metric = 'y';
    expect(listTemplates()[0]!.title).not.toBe('x');
    expect(listTemplates()[0]!.objective.metric).not.toBe('y');
  });

  it('the embedded files are in sync with packages/core/templates (run scripts/gen-templates.mjs)', () => {
    const fresh = generate() as string;
    expect(readFileSync(join(__dirname, 'files.generated.ts'), 'utf8')).toBe(fresh);
    expect(Object.keys(TEMPLATE_FILES).sort()).toEqual([...TEMPLATE_IDS].sort());
  });
});

describe.each(TEMPLATE_IDS)('template %s', (id) => {
  const rendered = renderTemplate(id);

  it('generates a ratchet.yaml that passes parseConfig', () => {
    const yaml = rendered.files.find((f) => f.path === 'ratchet.yaml')!.content;
    const r = parseConfig(yaml);
    if (!r.ok) throw new Error(r.errors.map((e) => `${e.path}: ${e.message}`).join('\n'));
    expect(r.config.evaluator.template).toBe(id);
    expect(r.config.evaluator.hardware).toBe(rendered.info.machine);
    expect(r.config.evaluator.kind).toBe(rendered.info.evaluatorKind);
    expect(r.config.objective).toMatchObject({ metric: rendered.info.objective.metric, direction: rendered.info.objective.direction });
    expect(r.config.objective.min_gain).toBeUndefined();
    expect(r.config.evaluator.repeats).toBeUndefined();
    expect(r.config.objective.value?.basis).toBe('estimated');
    expect(r.config.guardrails.length).toBeGreaterThan(0);
    expect(r.config.agent.instructions).toBe('RATCHET.md');
  });

  it('writes the evaluator files, the brief, and nothing outside the repo', () => {
    const paths = rendered.files.map((f) => f.path);
    expect(paths).toEqual(templateFilePaths(id));
    expect(paths).toEqual(rendered.info.files);
    for (const p of paths) expect(p.startsWith('/') || p.includes('..')).toBe(false);
    const brief = rendered.files.find((f) => f.path === 'RATCHET.md')!.content;
    expect(brief).not.toContain('{{');
    expect(brief).toContain(rendered.info.objective.metric);
    expect(brief).toContain('Ratchet-Candidate: true');
  });

  it('every evaluator file is protected automatically', () => {
    const cfg = parseConfig(rendered.files[0]!.content);
    if (!cfg.ok) throw new Error('invalid');
    const eff = effectiveProtected(cfg.config);
    const evaluatorFiles = rendered.files.map((f) => f.path).filter((p) => /^(bench|eval|data)\//.test(p));
    expect(evaluatorFiles.length).toBeGreaterThan(0);
    expect(findProtectedViolations(evaluatorFiles, eff.globs).map((v) => v.path)).toEqual(evaluatorFiles);
    expect(findProtectedViolations(['ratchet.yaml', 'pnpm-lock.yaml'], eff.globs)).toHaveLength(2);
    // the code the agents are meant to change stays editable
    expect(findProtectedViolations(['src/app.ts', 'src/forecast.mjs', 'src/task.mjs', 'RATCHET.md'], eff.globs)).toEqual([]);
  });

  it('works without the template: the same run command as the config resolves to', () => {
    const cfg = parseConfig(rendered.files[0]!.content);
    if (!cfg.ok) throw new Error('invalid');
    const script = cfg.config.evaluator.run.split(' ')[1]!;
    expect(rendered.files.map((f) => f.path)).toContain(script);
  });

  it('honours --metric, --direction and --value-usd and quotes what needs it', () => {
    const r = renderTemplate(id, { metric: 'a: b', direction: 'maximize', valueUsd: 7 });
    const parsed = parseConfig(r.files[0]!.content);
    if (!parsed.ok) throw new Error(JSON.stringify(parsed.errors));
    expect(parsed.config.objective).toMatchObject({ metric: 'a: b', direction: 'maximize', value: { usd_per_unit: 7 } });
    expect(r.files.find((f) => f.path === 'RATCHET.md')!.content).toContain('raise `a: b`');
  });

  it('the PR proposal says what was added and how to run a baseline', () => {
    const p = buildTemplateProposal(id);
    expect(p.branch).toBe(`ratchet/template-${id}`);
    expect(p.files).toEqual(rendered.files);
    expect(p.title).toContain(rendered.info.title);
    for (const f of rendered.files) expect(p.body).toContain(`\`${f.path}\``);
    expect(p.body).toContain('ratchet baseline');
    expect(p.body).toContain('Run a baseline');
    expect(p.body).toContain(rendered.info.objective.metric);
    expect(p.body).toContain('protected automatically');
  });
});

describe('legacy pack aliases', () => {
  it('perf and forecasting render the templates that replaced them', () => {
    expect(renderTemplate('perf').info.id).toBe('cli-cold-start');
    expect(renderTemplate('forecasting').info.id).toBe('forecast-backtest');
    expect(renderTemplate('perf').files[0]!.content).toContain('pack: perf');
  });
});

describe('the evaluator scripts run, in a fresh directory, and report the default metric', () => {
  const metricOf = (json: Record<string, unknown>, id: string) => {
    const v = lookupPath(json, getTemplate(id)!.objective.metric);
    expect(typeof v).toBe('number');
    expect(Number.isFinite(v as number)).toBe(true);
    return v as number;
  };

  it('http-latency: drives a service and reports percentiles and the error rate', async () => {
    const r = renderTemplate('http-latency');
    const dir = write(r);
    writeFileSync(
      join(dir, 'server.mjs'),
      `import http from 'node:http';
http.createServer((req, res) => { if (req.url === '/boom') { res.statusCode = 500; res.end('x'); return; } res.end('ok'); }).listen(Number(process.env.PORT));\n`,
    );
    const port = 41000 + Math.floor(Math.random() * 2000);
    writeFileSync(
      join(dir, 'bench/http-latency.config.json'),
      JSON.stringify({ start: 'node server.mjs', port, ready_path: '/', rate: 40, duration_s: 1, warmup_s: 0, requests: [{ path: '/' }, { path: '/x' }] }),
    );
    const json = await runEvaluator(dir, 'node bench/http-latency.mjs');
    expect(metricOf(json, 'http-latency')).toBeGreaterThan(0);
    expect(json.error_rate).toBe(0);
    expect((json.requests as number) >= 30).toBe(true);
    expect(lookupPath(json, 'http_req_duration.p99')).toBeGreaterThanOrEqual(metricOf(json, 'http-latency'));

    // the sealed holdout replaces the visible mix; a failing path shows up in error_rate
    writeFileSync(join(dir, 'holdout.json'), JSON.stringify({ requests: [{ path: '/boom' }] }));
    const held = await runEvaluator(dir, 'node bench/http-latency.mjs', { RATCHET_TRACE: join(dir, 'holdout.json') });
    expect(held.error_rate).toBe(1);
  }, 60_000);

  it('cli-cold-start: times invocations, counts unexpected exits, reads the holdout invocations', async () => {
    const r = renderTemplate('cli-cold-start');
    const dir = write(r);
    const env = { BENCH_RUNS: '3', BENCH_WARMUP: '0', BENCH_NO_HYPERFINE: '1' };
    const json = await runEvaluator(dir, 'node bench/cold-start.mjs', env);
    expect(metricOf(json, 'cli-cold-start')).toBeGreaterThan(0);
    expect(json).toMatchObject({ invocations: 1, runs: 3, error_rate: 0 });
    writeFileSync(join(dir, 'holdout.json'), JSON.stringify({ commands: ['node --version', 'node -e "process.exit(3)"'] }));
    const held = await runEvaluator(dir, 'node bench/cold-start.mjs', { ...env, RATCHET_TRACE: join(dir, 'holdout.json') });
    expect(held).toMatchObject({ invocations: 2, runs: 6, error_rate: 0.5 });
  }, 60_000);

  it('test-suite-time: times the test command and reports failures as error_rate', async () => {
    const dir = write(renderTemplate('test-suite-time'));
    const good = await runEvaluator(dir, 'node bench/test-time.mjs', { BENCH_TEST_COMMAND: 'node -e "setTimeout(()=>{},120)"' });
    expect(metricOf(good, 'test-suite-time')).toBeGreaterThanOrEqual(100);
    expect(good).toMatchObject({ error_rate: 0, exit_code: 0, runs: 1 });
    const bad = await runEvaluator(dir, 'node bench/test-time.mjs', { BENCH_TEST_COMMAND: 'node -e "process.exit(1)"' });
    expect(bad).toMatchObject({ error_rate: 1, exit_code: 1 });
  }, 60_000);

  it('build-size: runs the build, sizes the output, reports a failed build', async () => {
    const dir = write(renderTemplate('build-size'));
    writeFileSync(
      join(dir, 'bench/build-size.config.json'),
      JSON.stringify({
        clean: 'rm -rf out',
        build: `node -e "require('fs').mkdirSync('out/sub',{recursive:true});require('fs').writeFileSync('out/a.js','x'.repeat(2048));require('fs').writeFileSync('out/sub/b.css','y'.repeat(1024));require('fs').writeFileSync('out/ignore.png','z'.repeat(9999))"`,
        output_dir: 'out',
        include: ['js', 'css'],
      }),
    );
    const json = await runEvaluator(dir, 'node bench/build-size.mjs');
    expect(metricOf(json, 'build-size')).toBe(3);
    expect(json).toMatchObject({ files: 2, error_rate: 0 });
    expect(json.bundle_gzip_kb as number).toBeLessThan(3);
    expect(json.build_seconds as number).toBeGreaterThan(0);
    const failed = await runEvaluator(dir, 'node bench/build-size.mjs', { BENCH_BUILD_COMMAND: 'node -e "process.exit(2)"' });
    expect(failed).toMatchObject({ error_rate: 1, bundle_kb: 0 });
  }, 60_000);

  it('memory: reports the peak resident memory of the workload tree', async () => {
    const dir = write(renderTemplate('memory'));
    const env = { BENCH_RUNS: '2' };
    const small = await runEvaluator(dir, 'node bench/memory.mjs', { ...env, BENCH_COMMAND: 'node -e "0"' });
    const big = await runEvaluator(dir, 'node bench/memory.mjs', {
      ...env,
      BENCH_COMMAND: `node -e "const b=Buffer.alloc(200*1024*1024,1);let s=0;for(let i=0;i<b.length;i+=4096)s+=b[i];setTimeout(()=>{},150)"`,
    });
    expect(metricOf(small, 'memory')).toBeGreaterThan(5);
    expect(metricOf(big, 'memory')).toBeGreaterThan(metricOf(small, 'memory') + 100);
    expect(big).toMatchObject({ runs: 2, error_rate: 0 });
    expect((big.peak_rss_mb_runs as number[]).length).toBe(2);
    const failing = await runEvaluator(dir, 'node bench/memory.mjs', { ...env, BENCH_COMMAND: 'node -e "process.exit(1)"' });
    expect(failing.error_rate).toBe(1);
  }, 60_000);

  it('memory: hands the workload path (the holdout when RATCHET_TRACE is set) to the command as $WORKLOAD', async () => {
    const dir = write(renderTemplate('memory'));
    writeFileSync(join(dir, 'holdout.json'), '{}');
    const cmd = `node -e "process.exit(process.env.WORKLOAD.endsWith('holdout.json') ? 0 : 1)"`;
    expect((await runEvaluator(dir, 'node bench/memory.mjs', { BENCH_RUNS: '1', BENCH_COMMAND: cmd, RATCHET_TRACE: join(dir, 'holdout.json') })).error_rate).toBe(0);
    expect((await runEvaluator(dir, 'node bench/memory.mjs', { BENCH_RUNS: '1', BENCH_COMMAND: cmd })).error_rate).toBe(1);
  }, 60_000);

  it('forecast-backtest: deterministic, plausible WAPE, scores the sealed holdout, no look-ahead', async () => {
    const dir = write(renderTemplate('forecast-backtest'));
    const a = await runEvaluator(dir, 'node eval/backtest.mjs');
    const b = await runEvaluator(dir, 'node eval/backtest.mjs');
    expect(a).toEqual(b);
    expect(metricOf(a, 'forecast-backtest')).toBeGreaterThan(1);
    expect(metricOf(a, 'forecast-backtest')).toBeLessThan(100);
    expect(a).toMatchObject({ leakage_violations: 0, series: 20, origins: 100 });
    expect(a.mape as number).toBeGreaterThan(0);

    // the model only ever sees the history before the origin
    writeFileSync(
      join(dir, 'src/forecast.mjs'),
      `export function forecast(history, horizon) { if (!Object.isFrozen(history)) throw new Error('mutable'); return Array.from({ length: horizon }, () => history.length); }\n`,
    );
    await expect(runEvaluator(dir, 'node eval/backtest.mjs')).resolves.toHaveProperty('wape');

    writeFileSync(join(dir, 'holdout.json'), JSON.stringify([Array.from({ length: 40 }, (_, i) => 10 + (i % 5))]));
    const held = await runEvaluator(dir, 'node eval/backtest.mjs', { RATCHET_TRACE: join(dir, 'holdout.json') });
    expect(held).toMatchObject({ series: 1, origins: 3 });
  }, 60_000);

  it('llm-eval: scores the eval set, reports cost, reads the hidden set, counts failures', async () => {
    const dir = write(renderTemplate('llm-eval'));
    const json = await runEvaluator(dir, 'node eval/llm-eval.mjs');
    const score = metricOf(json, 'llm-eval');
    expect(score).toBeGreaterThan(0.3);
    expect(score).toBeLessThan(1);
    expect(json).toMatchObject({ n: 20, error_rate: 0, cost_per_call_usd: 0.0001 });
    expect(json.tokens_per_call as number).toBeGreaterThan(0);

    writeFileSync(join(dir, 'src/task.mjs'), 'export async function run(input) { return { output: input }; }\n');
    writeFileSync(
      join(dir, 'holdout.jsonl'),
      [
        { input: 'positive', expected: 'positive' },
        { input: 'The answer is 41.9', expected: 42, match: 'number' },
        { input: 'abc', expected: '^zzz', match: 'regex' },
        { input: 'x', expected: 'x', match: 'bogus' },
      ]
        .map((c) => JSON.stringify(c))
        .join('\n'),
    );
    const held = await runEvaluator(dir, 'node eval/llm-eval.mjs', { RATCHET_TRACE: join(dir, 'holdout.jsonl') });
    expect(held).toMatchObject({ n: 4, error_rate: 0.25, score: 0.5 });

    writeFileSync(join(dir, 'src/task.mjs'), `export async function run() { throw new Error('model down'); }\n`);
    expect((await runEvaluator(dir, 'node eval/llm-eval.mjs')).error_rate).toBe(1);
  }, 60_000);
});
