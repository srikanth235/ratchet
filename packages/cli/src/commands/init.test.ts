import { execFileSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { loadConfig } from '@ratchet/core';
import { bufferIo } from '../io.js';
import { runCli } from '../program.js';
import { PACKS } from './init-templates.js';
import { runInit } from './init.js';
import { runValidate } from './validate.js';

const tmp = () => mkdtempSync(join(tmpdir(), 'ratchet-init-'));

describe('ratchet init', () => {
  it('lists the packs', () => {
    const io = bufferIo();
    expect(runInit({ listPacks: true }, io)).toBe(0);
    for (const p of PACKS) expect(io.stdout).toContain(p.id);
    expect(io.stdout).toContain('bench/run.mjs');
    expect(io.stdout).toContain('eval/backtest.mjs');
  });

  it.each(PACKS.map((p) => p.id))('%s: writes files that pass `ratchet validate`', (pack) => {
    const dir = tmp();
    const io = bufferIo();
    expect(runInit({ pack, dir }, io)).toBe(0);
    for (const f of PACKS.find((p) => p.id === pack)!.render(PACKS.find((p) => p.id === pack)!.defaults)) {
      expect(existsSync(join(dir, f.path))).toBe(true);
    }
    const v = bufferIo();
    expect(runValidate(join(dir, 'ratchet.yaml'), v)).toBe(0);
    expect(v.stderr).toBe('');
    expect(v.stdout).toContain(`pack        ${pack}`);
    expect(v.stdout).toContain('(estimated)');
    const res = loadConfig(join(dir, 'ratchet.yaml'));
    expect(res.ok && res.config.agent.instructions).toBe('RATCHET.md');
    expect(existsSync(join(dir, 'RATCHET.md'))).toBe(true);
  });

  it('honors --metric, --direction and --value-usd', () => {
    const dir = tmp();
    expect(runInit({ pack: 'perf', dir, metric: 'throughput.rps', direction: 'maximize', valueUsd: 250 }, bufferIo())).toBe(0);
    const res = loadConfig(join(dir, 'ratchet.yaml'));
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    expect(res.config.objective).toMatchObject({ metric: 'throughput.rps', direction: 'maximize', value: { usd_per_unit: 250, basis: 'estimated' } });
  });

  it('quotes a metric that needs it, and the result still validates', () => {
    const dir = tmp();
    expect(runInit({ pack: 'forecasting', dir, metric: 'wape: p50' }, bufferIo())).toBe(0);
    const res = loadConfig(join(dir, 'ratchet.yaml'));
    expect(res.ok && res.config.objective.metric).toBe('wape: p50');
  });

  it('refuses to overwrite without --force, and writes nothing then', () => {
    const dir = tmp();
    writeFileSync(join(dir, 'RATCHET.md'), 'mine');
    const io = bufferIo();
    expect(runInit({ pack: 'perf', dir }, io)).toBe(1);
    expect(io.stderr).toContain('refusing to overwrite RATCHET.md');
    expect(readdirSync(dir)).toEqual(['RATCHET.md']);
    expect(readFileSync(join(dir, 'RATCHET.md'), 'utf8')).toBe('mine');

    const forced = bufferIo();
    expect(runInit({ pack: 'perf', dir, force: true }, forced)).toBe(0);
    expect(forced.stdout).toContain('RATCHET.md (overwritten)');
    expect(readFileSync(join(dir, 'RATCHET.md'), 'utf8')).toContain('Ratchet brief');
  });

  it('--dry-run prints the files and writes nothing', () => {
    const dir = tmp();
    const io = bufferIo();
    expect(runInit({ pack: 'forecasting', dir, dryRun: true }, io)).toBe(0);
    expect(io.stdout).toContain('would write eval/backtest.mjs');
    expect(io.stdout).toContain('pack: forecasting');
    expect(readdirSync(dir)).toEqual([]);
  });

  it('rejects bad input', () => {
    const dir = tmp();
    const cases: [Parameters<typeof runInit>[0], string][] = [
      [{ dir }, '--pack is required'],
      [{ pack: 'nope', dir }, 'unknown pack "nope"'],
      [{ pack: 'perf', dir, direction: 'sideways' }, '--direction must be'],
      [{ pack: 'perf', dir, valueUsd: -3 }, '--value-usd must be a positive number'],
    ];
    for (const [args, message] of cases) {
      const io = bufferIo();
      expect(runInit(args, io)).toBe(1);
      expect(io.stderr).toContain(message);
    }
    expect(readdirSync(dir)).toEqual([]);
  });

  it('the perf bench stub runs and reports the objective metric', () => {
    const dir = tmp();
    runInit({ pack: 'perf', dir }, bufferIo());
    const out = execFileSync(process.execPath, ['bench/run.mjs'], { cwd: dir, env: { ...process.env, BENCH_RUNS: '3', BENCH_COMMAND: 'node --version' }, encoding: 'utf8' });
    const json = JSON.parse(out) as { latency_ms: { p95: number }; error_rate: number };
    expect(json.latency_ms.p95).toBeGreaterThan(0);
    expect(json.error_rate).toBe(0);
  });

  it('the perf bench stub fills a custom metric path', () => {
    const dir = tmp();
    runInit({ pack: 'perf', dir, metric: 'startup.p95' }, bufferIo());
    const out = execFileSync(process.execPath, ['bench/run.mjs'], { cwd: dir, env: { ...process.env, BENCH_RUNS: '2' }, encoding: 'utf8' });
    expect((JSON.parse(out) as { startup: { p95: number } }).startup.p95).toBeGreaterThan(0);
  });

  it('the forecasting backtest stub runs, is deterministic, and the baseline model scores a plausible WAPE', () => {
    const dir = tmp();
    runInit({ pack: 'forecasting', dir }, bufferIo());
    writeFileSync(join(dir, 'package.json'), '{"type":"module"}');
    const run = () => execFileSync(process.execPath, ['eval/backtest.mjs'], { cwd: dir, encoding: 'utf8' });
    const a = JSON.parse(run()) as { wape: number; bias: number; leakage_violations: number };
    expect(run()).toBe(JSON.stringify(a) + '\n');
    expect(a.wape).toBeGreaterThan(1);
    expect(a.wape).toBeLessThan(100);
    expect(a.leakage_violations).toBe(0);
  });

  it('is wired into the program: `init --pack perf --dir`', async () => {
    const dir = tmp();
    const io = bufferIo();
    expect(await runCli(['init', '--pack', 'perf', '--dir', dir, '--value-usd', '5'], io)).toBe(0);
    expect(await runCli(['validate', join(dir, 'ratchet.yaml')], bufferIo())).toBe(0);
    expect(await runCli(['init', '--pack', 'perf', '--dir', dir], bufferIo())).toBe(1);
    expect(await runCli(['init', '--pack', 'perf', '--value-usd', 'abc'], bufferIo())).not.toBe(0);
    const list = bufferIo();
    expect(await runCli(['init', '--list-packs'], list)).toBe(0);
    expect(list.stdout).toContain('forecasting');
  });
});
