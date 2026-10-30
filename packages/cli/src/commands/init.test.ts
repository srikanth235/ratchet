import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { listTemplates, loadConfig } from '@ratchet/core';
import { bufferIo } from '../io.js';
import { runCli } from '../program.js';
import { runInit } from './init.js';
import { runValidate } from './validate.js';

const tmp = () => mkdtempSync(join(tmpdir(), 'ratchet-init-'));
const TEMPLATES = listTemplates({ kind: 'template' });
const IDS = TEMPLATES.map((t) => t.id);

describe('ratchet init --list-templates', () => {
  it('lists the templates and, separately, the demos', () => {
    const io = bufferIo();
    expect(runInit({ listTemplates: true }, io)).toBe(0);
    for (const id of ['http-latency', 'cli-cold-start', 'test-suite-time', 'build-size', 'memory', 'forecast-backtest', 'llm-eval']) expect(io.stdout).toContain(id);
    expect(io.stdout).toContain('bench/http-latency.mjs');
    expect(io.stdout).toContain('eval/backtest.mjs');
    expect(io.stdout).toContain('holdout: hidden request mix');
    expect(io.stdout).toContain('no holdout');
    expect(io.stdout.indexOf('Try a demo')).toBeGreaterThan(io.stdout.indexOf('llm-eval'));
    for (const d of ['demo-search', 'demo-cli', 'demo-forecast']) expect(io.stdout).toContain(`${d}  [demo]`);
    expect(io.stdout).toContain('examples/demo-search');
  });

  it('--list-packs is the same listing', () => {
    const a = bufferIo();
    const b = bufferIo();
    runInit({ listPacks: true }, a);
    runInit({ listTemplates: true }, b);
    expect(a.stdout).toBe(b.stdout);
  });
});

describe('ratchet init --template', () => {
  it.each(IDS)('%s: writes files that pass `ratchet validate`', (template) => {
    const dir = tmp();
    const io = bufferIo();
    expect(runInit({ template, dir }, io)).toBe(0);
    const info = TEMPLATES.find((t) => t.id === template)!;
    for (const f of info.files) expect(existsSync(join(dir, f))).toBe(true);
    const v = bufferIo();
    expect(runValidate(join(dir, 'ratchet.yaml'), v)).toBe(0);
    expect(v.stderr).toBe('');
    expect(v.stdout).toContain(`pack        ${info.category}`);
    expect(v.stdout).toContain(`(template ${template})`);
    expect(v.stdout).toContain('min gain calibrated from the baseline');
    expect(v.stdout).toContain('repeats calibrated from the baseline');
    expect(v.stdout).toContain('automatic: ratchet.yaml,');
    expect(v.stdout).toContain('(estimated)');
    const res = loadConfig(join(dir, 'ratchet.yaml'));
    expect(res.ok && res.config.agent.instructions).toBe('RATCHET.md');
    expect(existsSync(join(dir, 'RATCHET.md'))).toBe(true);
  });

  it('honors --metric, --direction and --value-usd', () => {
    const dir = tmp();
    expect(runInit({ template: 'http-latency', dir, metric: 'rps', direction: 'maximize', valueUsd: 250 }, bufferIo())).toBe(0);
    const res = loadConfig(join(dir, 'ratchet.yaml'));
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    expect(res.config.objective).toMatchObject({ metric: 'rps', direction: 'maximize', value: { usd_per_unit: 250, basis: 'estimated' } });
  });

  it('quotes a metric that needs it, and the result still validates', () => {
    const dir = tmp();
    expect(runInit({ template: 'forecast-backtest', dir, metric: 'wape: p50' }, bufferIo())).toBe(0);
    const res = loadConfig(join(dir, 'ratchet.yaml'));
    expect(res.ok && res.config.objective.metric).toBe('wape: p50');
  });

  it('refuses to overwrite without --force, and writes nothing then', () => {
    const dir = tmp();
    writeFileSync(join(dir, 'RATCHET.md'), 'mine');
    const io = bufferIo();
    expect(runInit({ template: 'cli-cold-start', dir }, io)).toBe(1);
    expect(io.stderr).toContain('refusing to overwrite RATCHET.md');
    expect(readdirSync(dir)).toEqual(['RATCHET.md']);
    expect(readFileSync(join(dir, 'RATCHET.md'), 'utf8')).toBe('mine');

    const forced = bufferIo();
    expect(runInit({ template: 'cli-cold-start', dir, force: true }, forced)).toBe(0);
    expect(forced.stdout).toContain('RATCHET.md (overwritten)');
    expect(readFileSync(join(dir, 'RATCHET.md'), 'utf8')).toContain('Ratchet brief');
  });

  it('--dry-run prints the files and writes nothing', () => {
    const dir = tmp();
    const io = bufferIo();
    expect(runInit({ template: 'forecast-backtest', dir, dryRun: true }, io)).toBe(0);
    expect(io.stdout).toContain('would write eval/backtest.mjs');
    expect(io.stdout).toContain('pack: forecasting');
    expect(io.stdout).toContain('template: forecast-backtest');
    expect(readdirSync(dir)).toEqual([]);
  });

  it('rejects bad input', () => {
    const dir = tmp();
    const cases: [Parameters<typeof runInit>[0], string][] = [
      [{ dir }, '--template is required'],
      [{ template: 'nope', dir }, 'unknown template "nope"'],
      [{ template: 'demo-search', dir }, 'is a demo, not a template'],
      [{ template: 'memory', dir, direction: 'sideways' }, '--direction must be'],
      [{ template: 'memory', dir, valueUsd: -3 }, '--value-usd must be a positive number'],
    ];
    for (const [args, message] of cases) {
      const io = bufferIo();
      expect(runInit(args, io)).toBe(1);
      expect(io.stderr).toContain(message);
    }
    expect(readdirSync(dir)).toEqual([]);
  });
});

describe('ratchet init --pack (legacy alias)', () => {
  it('perf is the cli-cold-start template, forecasting is forecast-backtest, and the output says so', () => {
    const perf = tmp();
    const io = bufferIo();
    expect(runInit({ pack: 'perf', dir: perf }, io)).toBe(0);
    expect(io.stdout).toContain('ratchet init --template cli-cold-start (--pack perf is now the cli-cold-start template)');
    expect(existsSync(join(perf, 'bench/cold-start.mjs'))).toBe(true);
    const fc = tmp();
    expect(runInit({ pack: 'forecasting', dir: fc }, bufferIo())).toBe(0);
    expect(existsSync(join(fc, 'eval/backtest.mjs'))).toBe(true);
    expect(existsSync(join(fc, 'src/forecast.mjs'))).toBe(true);
  });

  it('--template wins over --pack, and a template name works as a pack', () => {
    const dir = tmp();
    expect(runInit({ pack: 'perf', template: 'memory', dir }, bufferIo())).toBe(0);
    expect(existsSync(join(dir, 'bench/memory.mjs'))).toBe(true);
    const dir2 = tmp();
    expect(runInit({ pack: 'llm-eval', dir: dir2 }, bufferIo())).toBe(0);
    expect(existsSync(join(dir2, 'eval/llm-eval.mjs'))).toBe(true);
  });
});

describe('the files a template writes actually run', () => {
  it('cli-cold-start: node bench/cold-start.mjs reports the objective metric', () => {
    const dir = tmp();
    runInit({ template: 'cli-cold-start', dir }, bufferIo());
    const out = execFileSync(process.execPath, ['bench/cold-start.mjs'], { cwd: dir, env: { ...process.env, BENCH_RUNS: '3', BENCH_WARMUP: '0', BENCH_NO_HYPERFINE: '1' }, encoding: 'utf8' });
    const json = JSON.parse(out) as { cold_start_ms: { p50: number }; error_rate: number };
    expect(json.cold_start_ms.p50).toBeGreaterThan(0);
    expect(json.error_rate).toBe(0);
  });

  it('forecast-backtest: deterministic, and the baseline model scores a plausible WAPE', () => {
    const dir = tmp();
    runInit({ template: 'forecast-backtest', dir }, bufferIo());
    const run = () => execFileSync(process.execPath, ['eval/backtest.mjs'], { cwd: dir, encoding: 'utf8' });
    const a = JSON.parse(run()) as { wape: number; leakage_violations: number };
    expect(run()).toBe(JSON.stringify(JSON.parse(run())) + '\n');
    expect(a.wape).toBeGreaterThan(1);
    expect(a.wape).toBeLessThan(100);
    expect(a.leakage_violations).toBe(0);
  });

  it('`ratchet baseline` runs a generated config end to end and explains the calibration', async () => {
    const dir = tmp();
    runInit({ template: 'forecast-backtest', dir }, bufferIo());
    const io = bufferIo();
    const code = await runCli(['baseline', join(dir, 'ratchet.yaml'), '--timeout', '60000'], io);
    expect(io.stderr).toBe('');
    expect(code).toBe(0);
    expect(io.stdout).toContain('Baseline: running `node eval/backtest.mjs` 5x');
    expect(io.stdout).toMatch(/calibration this metric is stable \(±0%\); wins must beat 1%/);
  }, 90_000);
});

describe('program wiring', () => {
  it('init --template / --pack / --list-templates / --list-packs', async () => {
    const dir = tmp();
    expect(await runCli(['init', '--template', 'http-latency', '--dir', dir, '--value-usd', '5'], bufferIo())).toBe(0);
    expect(await runCli(['validate', join(dir, 'ratchet.yaml')], bufferIo())).toBe(0);
    expect(await runCli(['init', '--template', 'http-latency', '--dir', dir], bufferIo())).toBe(1);
    expect(await runCli(['init', '--template', 'http-latency', '--value-usd', 'abc'], bufferIo())).not.toBe(0);
    const pack = tmp();
    expect(await runCli(['init', '--pack', 'perf', '--dir', pack], bufferIo())).toBe(0);
    const list = bufferIo();
    expect(await runCli(['init', '--list-templates'], list)).toBe(0);
    expect(list.stdout).toContain('forecast-backtest');
    const legacy = bufferIo();
    expect(await runCli(['init', '--list-packs'], legacy)).toBe(0);
    expect(legacy.stdout).toBe(list.stdout);
  });
});
