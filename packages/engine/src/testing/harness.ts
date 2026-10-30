import { openLedger, type AgentKind, type Ledger } from '@ratchet/core';
import { Engine } from '../engine.js';
import type { EngineSettings } from '../settings.js';
import {
  FakeArtifacts,
  FakeBench,
  FakeClock,
  FakeCompute,
  FakeGit,
  FakePrices,
  FakeRunners,
  FakeSandbox,
  type CallLog,
} from './fakes.js';

export const CONFIG_YAML = `version: 0
objective:
  metric: latency_ms
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: bench.sh
  repeats: 3
  hardware: c7i.2xlarge
  confirm_trace: s3://sealed/trace.jsonl
guardrails:
  - run: pytest -q
  - peak_rss: "< 1.2x"
protected:
  - bench/**
  - ratchet.yaml
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.xlarge]
`;

export const TICK_MS = 60_000;

export interface Harness {
  log: CallLog;
  clock: FakeClock;
  ledger: Ledger;
  git: FakeGit;
  bench: FakeBench;
  runners: FakeRunners;
  compute: FakeCompute;
  prices: FakePrices;
  artifacts: FakeArtifacts;
  engine: Engine;
  settings: EngineSettings;
  events: unknown[];
  /** A fresh Engine on the same fakes: a process restart. `reopenLedger` also closes and reopens the SQLite file. */
  restart(reopenLedger?: boolean): Engine;
  /** Advance the clock one tick, tick, and wait for the bench job started (if any). */
  cycle(n?: number): Promise<void>;
  until(pred: () => boolean, max?: number): Promise<void>;
  /** Create + start an optimization; returns [optId, agentIds]. */
  begin(opts?: BeginOptions): Promise<{ id: string; agents: Record<string, string> }>;
}

export interface BeginOptions {
  agents?: AgentKind[];
  budgetUsd?: number;
  timeLimitSeconds?: number;
  plateauEvals?: number;
  baseline?: number[];
  confirmBaseline?: number[];
}

export interface HarnessOptions {
  ledgerPath?: string;
  hourly?: number;
  settings?: EngineSettings;
  configYaml?: string;
}

export function makeHarness(opts: HarnessOptions = {}): Harness {
  const log: CallLog = [];
  const clock = new FakeClock();
  let ledger = openLedger(opts.ledgerPath ?? ':memory:', { now: () => clock.now() });
  const git = new FakeGit(log, clock, { 'ratchet.yaml': opts.configYaml ?? CONFIG_YAML, 'RATCHET.md': '# Brief\nMake it faster.\n' });
  const bench = new FakeBench(log, clock);
  const runners = new FakeRunners(log);
  const compute = new FakeCompute(log, clock);
  const prices = new FakePrices({}, opts.hourly ?? 1);
  const artifacts = new FakeArtifacts();
  const settings: EngineSettings = {
    tickIntervalMs: TICK_MS,
    secrets: (_kind, name) => `value-of-${name}`,
    newId: () => 'opt-test',
    ...opts.settings,
  };
  const events: unknown[] = [];

  const build = (): Engine => {
    const e = new Engine({
      ledger,
      mode: 'simulation',
      clock,
      ports: { sandbox: new FakeSandbox(), compute, prices, artifacts, git, runners, bench },
      settings,
    });
    e.subscribe((ev) => events.push(ev));
    return e;
  };

  const h: Harness = {
    log, clock, ledger, git, bench, runners, compute, prices, artifacts, settings, events,
    engine: build(),
    restart(reopenLedger = false) {
      void h.engine.close();
      if (reopenLedger) {
        if (!opts.ledgerPath) throw new Error('reopenLedger needs ledgerPath');
        h.ledger.close();
        ledger = openLedger(opts.ledgerPath, { now: () => clock.now() });
        h.ledger = ledger;
      }
      h.engine = build();
      return h.engine;
    },
    async cycle(n = 1) {
      for (let i = 0; i < n; i++) {
        clock.advance(TICK_MS);
        await h.engine.tick();
        await h.engine.settle();
      }
    },
    async until(pred, max = 60) {
      for (let i = 0; i < max; i++) {
        if (pred()) return;
        await h.cycle();
      }
      if (!pred()) throw new Error(`condition not met after ${max} cycles`);
    },
    async begin(b = {}) {
      bench.set('main-sha', 'baseline', { samples: b.baseline ?? [100, 102, 98], metrics: { peak_rss: 500 } });
      bench.set('main-sha', 'confirm', { samples: b.confirmBaseline ?? [100, 100, 100] });
      const id = await h.engine.createOptimization({
        repoUrl: git.remoteUrl(),
        baseRef: 'main',
        agents: b.agents ?? ['claude-code', 'codex'],
        budgetUsd: b.budgetUsd ?? 100,
        timeLimitSeconds: b.timeLimitSeconds ?? 8 * 3600,
        plateauEvals: b.plateauEvals ?? 50,
      });
      await h.engine.start(id);
      const agents = Object.fromEntries(ledger.listAgents(id).map((a) => [a.kind, a.id]));
      return { id, agents };
    },
  };
  return h;
}
