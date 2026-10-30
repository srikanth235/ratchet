#!/usr/bin/env node
/**
 * Run a full Ratchet optimization in simulation mode against a shipped demo.
 *
 *   node packages/runtime/dist/bin/simulate.js [--project search|forecast] [--attempts N] [--duration S] [--repeats N] [--minutes M]
 *                                              [--db <path>] [--summary <path.json>]
 *
 * `--project search` (default): examples/demo-search, perf pack. `--project forecast`: examples/demo-forecast,
 * forecasting pack. Build first (`pnpm build`). Prints engine events and a final summary with the $ value of the win
 * (core's estimateValue / returnOnSpend); exits non-zero if no verified win.
 *
 * The scratch directory (`.ratchet-sim-*` in the repo root, holding the git remote, artifacts and PR) is never
 * deleted. The ledger lives there too unless `--db <path>` puts it somewhere that outlives the scratch dir.
 * `--summary <path.json>` writes the machine-readable run summary (see summary.ts) in addition to printing it.
 */
import { mkdirSync, mkdtempSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { estimateValue, openLedger, returnOnSpend } from '@ratchet/core';
import type { EngineSettings } from '@ratchet/engine';
import { SIMULATION_PROJECTS, createSimulationRuntime, type SimulationProject } from '../simulation.js';
import { buildSimulationSummary, writeSimulationSummary } from '../summary.js';

const arg = (name: string, def: number) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? Number(process.argv[i + 1]) : def;
};
const strArg = (name: string): string | undefined => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : undefined;
};

export async function simulate(o: { repoRoot: string; dbPath?: string; project?: SimulationProject; attempts?: number; durationSec?: number; repeats?: number; minutes?: number; settings?: EngineSettings; log?: (s: string) => void }) {
  const log = o.log ?? ((s: string) => console.log(`${new Date().toISOString().slice(11, 19)} ${s}`));
  const workRoot = mkdtempSync(join(o.repoRoot, '.ratchet-sim-'));
  const dbPath = o.dbPath ? resolve(o.dbPath) : join(workRoot, 'ratchet.db');
  mkdirSync(dirname(dbPath), { recursive: true });
  const ledger = openLedger(dbPath);
  const project = o.project ?? 'search';
  const trim = (v: 'cc' | 'cx') => {
    const s = SIMULATION_PROJECTS[project].scripts(v);
    return o.attempts ? { ...s, attempts: s.attempts.slice(0, o.attempts) } : s;
  };
  const rt = await createSimulationRuntime({
    ledger,
    workRoot,
    repoRoot: o.repoRoot,
    project,
    benchDurationSec: o.durationSec ?? 3,
    repeats: o.repeats ?? 2,
    scripts: { 'claude-code': trim('cc'), codex: trim('cx') },
    ...(o.settings ? { settings: o.settings } : {}),
  });
  const unsub = rt.engine.subscribe((e) => {
    if (e.type === 'spend') return; // one per metering tick: too noisy for the console
    if (e.type === 'agent-event') {
      if (['commit', 'verdict', 'error'].includes(e.event.type)) log(`  [${e.agentId}] ${e.event.type}: ${e.event.text ?? ''}`);
    } else log(`${e.type}: ${JSON.stringify(e).slice(0, 200)}`);
  });
  const id = await rt.engine.createOptimization({
    name: `${rt.config.objective.name ?? rt.config.objective.metric} (simulation)`,
    repoUrl: rt.repoUrl,
    baseRef: rt.baseRef,
    agents: ['claude-code', 'codex'],
    budgetUsd: 50,
    timeLimitSeconds: (o.minutes ?? 20) * 60,
    plateauEvals: 50,
  });
  const ac = new AbortController();
  const running = rt.engine.run(ac.signal);
  await rt.engine.start(id);
  const deadline = Date.now() + (o.minutes ?? 20) * 60_000 + 120_000;
  for (;;) {
    const opt = ledger.getOptimization(id);
    if (opt && ['completed', 'stopped', 'failed'].includes(opt.status)) break;
    if (Date.now() > deadline) {
      log('simulation timed out; stopping');
      await rt.engine.stop(id, 'simulation timeout');
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  const summarize = () => ledger.listOptimizations().find((r) => r.id === id)!;
  const wins = summarize().verifiedWins;
  const history = ledger.ratchetHistory(id);
  const candidates = ledger.candidatesWithResults(id);
  let pr: string | undefined;
  if (wins > 0) pr = (await rt.engine.openPullRequest(id)).url;
  await rt.engine.teardown(id).catch((err) => log(`teardown: ${(err as Error).message}`));
  ac.abort();
  await running.catch(() => {});
  unsub();
  await rt.dispose();
  // After teardown: final status (`completed`) and the spend including the last metering pass.
  const summary = summarize();
  const value = summary.baselineValue !== null && summary.bestValue !== null ? estimateValue(rt.config, summary.baselineValue, summary.bestValue) : null;
  const roi = value ? returnOnSpend(value.usdPerYear, summary.spendUsd) : null;
  return { id, summary, history, candidates, pr, workRoot, dbPath, ledger, compute: rt.compute, engine: rt.engine, config: rt.config, value, roi };
}

const isMain = process.argv[1] && resolve(process.argv[1]) === resolve(new URL(import.meta.url).pathname);
if (isMain) {
  const repoRoot = resolve(new URL('../../../../', import.meta.url).pathname);
  const project = (strArg('project') ?? 'search') as SimulationProject;
  if (!(project in SIMULATION_PROJECTS)) {
    console.error(`unknown --project ${project}; expected one of ${Object.keys(SIMULATION_PROJECTS).join(', ')}`);
    process.exit(2);
  }
  const r = await simulate({ repoRoot, project, ...(strArg('db') ? { dbPath: strArg('db')! } : {}), attempts: arg('attempts', 0) || undefined, durationSec: arg('duration', 3), repeats: arg('repeats', 2), minutes: arg('minutes', 20) });
  console.log('\n=== summary ===');
  const { metric, name } = r.config.objective;
  const summary = buildSimulationSummary({
    project,
    pack: r.config.pack,
    objective: name ?? metric,
    summary: r.summary,
    candidates: r.candidates,
    spend: r.ledger.spendBreakdown(r.id),
    value: r.value,
    roi: r.roi,
    pr: r.pr,
    workRoot: r.workRoot,
    db: r.dbPath,
  });
  console.log(JSON.stringify(summary, null, 2));
  const summaryPath = strArg('summary');
  if (summaryPath) console.log(`summary written to ${writeSimulationSummary(summaryPath, summary)}`);
  for (const c of r.candidates as any[]) {
    const bench = c.benchValue === null ? '' : ` bench=${c.benchValue}`;
    const confirm = c.confirmValue === null ? '' : ` confirm=${c.confirmValue}`;
    console.log(`  ${String(c.verdict).padEnd(15)} ${String(c.sha ?? '').slice(0, 8)}${bench}${confirm}  ${String(c.message ?? '').split('\n')[0]}${c.reasons?.length ? `\n${' '.repeat(18)}${c.reasons[0]}` : ''}`);
  }
  r.ledger.close();
  console.log(`ledger: ${r.dbPath}`);
  process.exit(r.summary.verifiedWins > 0 ? 0 : 1);
}
