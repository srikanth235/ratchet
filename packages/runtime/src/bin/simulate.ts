#!/usr/bin/env node
/**
 * Run a full Ratchet optimization in simulation mode against examples/demo-search.
 *
 *   node packages/runtime/dist/bin/simulate.js [--attempts N] [--duration S] [--repeats N] [--minutes M]
 *
 * Prints engine events and a final summary; exits non-zero if no verified win.
 */
import { mkdtempSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { openLedger } from '@ratchet/core';
import { loadDemoSearchScript } from '@ratchet/agents';
import { createSimulationRuntime } from '../simulation.js';

const arg = (name: string, def: number) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? Number(process.argv[i + 1]) : def;
};

export async function simulate(o: { repoRoot: string; attempts?: number; durationSec?: number; repeats?: number; minutes?: number; log?: (s: string) => void }) {
  const log = o.log ?? ((s: string) => console.log(s));
  const workRoot = mkdtempSync(join(o.repoRoot, '.ratchet-sim-'));
  const ledger = openLedger(join(workRoot, 'ratchet.db'));
  const trim = (v: 'cc' | 'cx') => {
    const s = loadDemoSearchScript(v);
    return o.attempts ? { ...s, attempts: s.attempts.slice(0, o.attempts) } : s;
  };
  const rt = await createSimulationRuntime({
    ledger,
    workRoot,
    repoRoot: o.repoRoot,
    benchDurationSec: o.durationSec ?? 3,
    repeats: o.repeats ?? 2,
    scripts: { 'claude-code': trim('cc'), codex: trim('cx') },
  });
  const unsub = rt.engine.subscribe((e) => {
    if (e.type === 'agent-event') {
      if (['commit', 'verdict', 'error'].includes(e.event.type)) log(`  [${e.agentId}] ${e.event.type}: ${e.event.text ?? ''}`);
    } else log(`${e.type}: ${JSON.stringify(e).slice(0, 200)}`);
  });
  const id = await rt.engine.createOptimization({
    name: 'search-api p95 (simulation)',
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
  const summary = ledger.listOptimizations().find((r) => r.id === id)!;
  const history = ledger.ratchetHistory(id);
  const candidates = ledger.candidatesWithResults(id);
  let pr: string | undefined;
  if (summary.verifiedWins > 0) pr = (await rt.engine.openPullRequest(id)).url;
  await rt.engine.teardown(id).catch((err) => log(`teardown: ${(err as Error).message}`));
  ac.abort();
  await running.catch(() => {});
  unsub();
  await rt.dispose();
  return { id, summary, history, candidates, pr, workRoot, ledger, compute: rt.compute, engine: rt.engine };
}

const isMain = process.argv[1] && resolve(process.argv[1]) === resolve(new URL(import.meta.url).pathname);
if (isMain) {
  const repoRoot = resolve(new URL('../../../../', import.meta.url).pathname);
  const r = await simulate({ repoRoot, attempts: arg('attempts', 0) || undefined, durationSec: arg('duration', 3), repeats: arg('repeats', 2), minutes: arg('minutes', 20) });
  console.log('\n=== summary ===');
  console.log(JSON.stringify({ status: r.summary.status, baseline: r.summary.baselineValue, best: r.summary.bestValue, gainPct: r.summary.gainPct, wins: r.summary.verifiedWins, candidates: r.summary.candidateCount, spendUsd: r.summary.spendUsd, pr: r.pr, workRoot: r.workRoot }, null, 2));
  for (const c of r.candidates) console.log(`  ${String((c as any).verdict).padEnd(15)} ${(c as any).sha?.slice(0, 8)} ${(c as any).message ?? ''}`);
  process.exit(r.summary.verifiedWins > 0 ? 0 : 1);
}
