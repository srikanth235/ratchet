import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { openLedger, type CostActuals, type CostReconciler } from '@ratchet/core';
import { bufferIo } from '../io.js';
import { runCli } from '../program.js';

const HOUR = 3_600_000;
const T0 = 1_700_000_000_000;
let dir: string;
let db: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'ratchet-cost-'));
  db = join(dir, 'ratchet.db');
  const ledger = openLedger(db);
  ledger.createOptimization({ id: 'opt-1', repo: 'a/b', baseRef: 'main', metric: 'p95', direction: 'minimize', minGain: 0.03, budgetUsd: 10, timeLimitS: 60, configYaml: 'v' });
  ledger.setOptimizationStatus('opt-1', 'running', T0);
  ledger.recordSpend({ optimizationId: 'opt-1', category: 'agent_runner', usd: 1, source: 'ec2:i-1', at: T0 + 1 });
  ledger.setOptimizationStatus('opt-1', 'completed', T0 + HOUR);
  ledger.close();
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const reconciler = (a: CostActuals): CostReconciler => ({ actuals: async () => a });
const rows = () => {
  const l = openLedger(db);
  try {
    return l.sqlite.prepare("select category, usd, estimated, source from spend_events where source = 'cost-explorer'").all();
  } finally {
    l.close();
  }
};
const later = { now: () => T0 + 30 * HOUR };

describe('ratchet cost reconcile', () => {
  it('writes corrections, prints a table, and is idempotent', async () => {
    const deps = { ...later, reconciler: () => reconciler({ byCategory: { agent_runner: 1.25, bench_runner: 0.5 }, groups: 2, complete: true }) };
    const io = bufferIo();
    expect(await runCli(['cost', 'reconcile', 'opt-1', '--db', db], io, deps)).toBe(0);
    expect(io.stdout).toContain('reconciled: 2 correction row(s)');
    expect(io.stdout).toMatch(/agent_runner\s+\$1\.0000\s+\$1\.2500\s+\+\$0\.2500\s+written/);
    expect(rows()).toEqual([
      { category: 'agent_runner', usd: 0.25, estimated: 0, source: 'cost-explorer' },
      { category: 'bench_runner', usd: 0.5, estimated: 0, source: 'cost-explorer' },
    ]);

    const again = bufferIo();
    expect(await runCli(['cost', 'reconcile', 'opt-1', '--db', db, '--json'], again, deps)).toBe(0);
    expect(JSON.parse(again.stdout)).toMatchObject({ status: 'unchanged', writtenUsd: 0 });
    expect(rows()).toHaveLength(2);
  });

  it('reports pending and writes nothing inside the billing lag', async () => {
    const io = bufferIo();
    const deps = { now: () => T0 + 2 * HOUR, reconciler: () => reconciler({ byCategory: { agent_runner: 9 }, groups: 1, complete: true }) };
    expect(await runCli(['cost', 'reconcile', 'opt-1', '--db', db], io, deps)).toBe(0);
    expect(io.stdout).toContain('pending: nothing was written');
    expect(rows()).toEqual([]);
    // --lag-hours 0 lifts the gate
    const io2 = bufferIo();
    expect(await runCli(['cost', 'reconcile', 'opt-1', '--db', db, '--lag-hours', '0'], io2, deps)).toBe(0);
    expect(io2.stdout).toContain('reconciled');
  });

  it('fails clearly: no ledger, missing file, unknown optimization, billing errors', async () => {
    const saved = process.env['RATCHET_DB'];
    delete process.env['RATCHET_DB'];
    try {
      const a = bufferIo();
      expect(await runCli(['cost', 'reconcile', 'opt-1'], a, later)).toBe(1);
      expect(a.stderr).toContain('--db');
      process.env['RATCHET_DB'] = db; // the env fallback works
      const env = bufferIo();
      expect(await runCli(['cost', 'reconcile', 'opt-1'], env, { ...later, reconciler: () => reconciler({ byCategory: { agent_runner: 1 }, groups: 1, complete: true }) })).toBe(0);
    } finally {
      if (saved === undefined) delete process.env['RATCHET_DB'];
      else process.env['RATCHET_DB'] = saved;
    }
    const b = bufferIo();
    expect(await runCli(['cost', 'reconcile', 'opt-1', '--db', join(dir, 'nope.db')], b, later)).toBe(1);
    expect(b.stderr).toContain('ledger not found');
    const c = bufferIo();
    expect(await runCli(['cost', 'reconcile', 'opt-zzz', '--db', db], c, later)).toBe(1);
    expect(c.stderr).toContain('not found');
    const d = bufferIo();
    const denied = { ...later, reconciler: () => ({ actuals: async () => { throw Object.assign(new Error('User is not authorized to perform: ce:GetCostAndUsage'), { name: 'AccessDeniedException' }); } }) };
    expect(await runCli(['cost', 'reconcile', 'opt-1', '--db', db], d, denied)).toBe(1);
    expect(d.stderr).toContain('docs/real-env/aws.md');
  });

  it('shows up in --help', async () => {
    const io = bufferIo();
    expect(await runCli(['cost', 'reconcile', '--help'], io)).toBe(0);
    expect(io.stdout).toContain('--db');
  });
});
