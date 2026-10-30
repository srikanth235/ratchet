/**
 * The "This machine" wiring end to end, without AWS: the real Engine, the real local agent control with the real Claude Code adapter
 * (driving a stand-in `claude` script), a git worktree per agent against a local "GitHub", and the automatic sandbox (mock provider)
 * created at the first run and removed after the idle window with a receipt.
 */
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { openLedger, type BenchOutcome, type BenchRunner, type EngineEvent, type RunnerControl } from '@ratchet/core';
import { Engine } from '@ratchet/engine';
import { FsArtifactStore, MockCloudCompute, MockSandboxProvider, StaticPriceBook } from '@ratchet/aws';
import { LocalGitHost, createBareRemote } from '@ratchet/github';
import { createLocalAgentControl, type HarnessInfo } from '@ratchet/agents';
import { systemClock } from './clock.js';

const roots: string[] = [];
afterEach(() => void roots.splice(0).forEach((r) => rmSync(r, { recursive: true, force: true })));

const CONFIG = `version: 0
objective:
  metric: latency_ms
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: bench.sh
  repeats: 3
  hardware: c7i.2xlarge
guardrails:
  - run: "true"
protected:
  - bench/**
  - ratchet.yaml
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.xlarge]
`;

const STREAM = [
  '{"type":"system","subtype":"init","session_id":"sess-1","model":"shim"}',
  '{"type":"assistant","message":{"content":[{"type":"text","text":"made it faster"}]}}',
  '{"type":"result","subtype":"success","is_error":false,"result":"done","num_turns":1,"duration_ms":5,"total_cost_usd":0.03,"usage":{"input_tokens":10,"output_tokens":5}}',
];

async function until(cond: () => boolean | Promise<boolean>, ms: number, what: string): Promise<void> {
  const t0 = Date.now();
  while (!(await cond())) {
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 25));
  }
}

describe('local agents through the real engine and an automatic sandbox', () => {
  it('runs a Claude Code agent in a worktree, verifies its commit, then removes the worktree and (after the idle window) the sandbox', async () => {
    const root = mkdtempSync(join(tmpdir(), 'ratchet-int-'));
    roots.push(root);
    const project = join(root, 'project');
    mkdirSync(join(project, 'src'), { recursive: true });
    writeFileSync(join(project, 'ratchet.yaml'), CONFIG);
    writeFileSync(join(project, 'RATCHET.md'), '# Brief\nMake it faster.\n');
    writeFileSync(join(project, 'src', 'app.txt'), 'slow\n');
    const remote = await createBareRemote(project, { remoteDir: join(root, 'remote.git'), workDir: join(root, 'seed') });
    const git = new LocalGitHost({ remoteDir: remote.remoteDir, prDir: join(root, 'prs') });

    // a stand-in for the user's `claude`: signed in, edits the worktree once, logs the key it was (not) given
    const bin = join(root, 'bin');
    mkdirSync(bin);
    const seen = join(root, 'seen.txt');
    writeFileSync(
      join(bin, 'claude'),
      [
        '#!/bin/sh',
        'case "$1" in --version) echo "2.1.4 (Claude Code)"; exit 0;; esac',
        'cat >/dev/null',
        `printf 'cwd=%s key=%s\\n' "$PWD" "$ANTHROPIC_API_KEY" >> ${JSON.stringify(seen)}`,
        'case "$*" in *--resume*) ;; *) echo fast > src/app.txt;; esac',
        ...STREAM.map((l) => `echo '${l}'`),
        '',
      ].join('\n'),
    );
    chmodSync(join(bin, 'claude'), 0o755);
    const harness: HarnessInfo = { kind: 'claude-code', installed: true, version: '2.1.4', path: join(bin, 'claude'), auth: 'signed-in', hint: '' };
    const agentsRoot = join(root, 'agents');
    const localRunners = createLocalAgentControl({
      rootDir: agentsRoot,
      harnesses: [harness],
      loginPath: '/usr/bin:/bin',
      baseEnv: { HOME: root, ANTHROPIC_API_KEY: 'sk-ant-MUSTNOTPASS' },
      loop: { maxIdleTurns: 1, retryDelayMs: 1 },
    });

    const bench: BenchRunner = {
      async run(job): Promise<BenchOutcome> {
        const v = job.phase === 'baseline' ? 100 : 80;
        const at = Date.now();
        return { runs: [v, v, v].map((value) => ({ value, metrics: { objective: value }, durationMs: 10 })), samples: [v, v, v], guardrails: [], startedAt: at, finishedAt: at + 10 };
      },
    };
    const noCloudRunners: RunnerControl = {
      start: async () => void Promise.reject(new Error('cloud runners must not be used')),
      send: async () => {},
      poll: async () => ({ events: [], cursor: '0' }),
      stop: async () => {},
    };
    const ledger = openLedger(':memory:');
    const compute = new MockCloudCompute({ clock: systemClock });
    const sandboxProvider = new MockSandboxProvider();
    const engine = new Engine({
      ledger,
      mode: 'live',
      clock: systemClock,
      ports: { sandbox: sandboxProvider, compute, prices: new StaticPriceBook(), artifacts: new FsArtifactStore(join(root, 'artifacts')), git, runners: noCloudRunners, localRunners, bench },
      settings: { tickIntervalMs: 40, scratchDir: join(root, 'scratch'), agentsWhere: 'local', sandbox: { auto: true, idleMs: 600, retryMs: 100 }, noise: { k: 0.25 } },
    });
    const events: EngineEvent[] = [];
    engine.subscribe((e) => events.push(e));
    const stop = new AbortController();
    const loop = engine.run(stop.signal);
    try {
      const id = await engine.createOptimization({ id: 'run-1', repoUrl: remote.remoteDir, baseRef: 'main', agents: ['claude-code'], budgetUsd: 20, timeLimitSeconds: 600, plateauEvals: 5 });
      expect(ledger.getOptimization(id)!.sandboxId).toBeNull();
      await engine.start(id);

      // the sandbox appeared with the run, and the agent runs on this machine
      expect(ledger.getOptimization(id)!.sandboxId).toBe('ratchet-sandbox');
      expect(ledger.getSandbox('ratchet-sandbox')).toMatchObject({ status: 'applied' });
      expect(engine.runActive()).toBe(true);
      const agent = ledger.listAgents(id)[0]!;
      expect(agent.runnerInstanceId).toBe(`local:${agent.id}`);
      expect(compute.instances.size).toBe(0);

      await until(() => ledger.getOptimization(id)!.status === 'stopped', 60_000, 'the run to finish by itself (all agents done)');
      const candidates = ledger.candidatesWithResults(id);
      expect(candidates.map((c) => c.verdict)).toEqual(['verified']);
      expect(ledger.currentBest(id)).toBe(80);
      expect(readFileSync(seen, 'utf8')).toContain(`cwd=${join(agentsRoot, id, agent.id)}`);
      expect(readFileSync(seen, 'utf8')).toMatch(/ key=\n/);
      expect(readFileSync(seen, 'utf8')).not.toContain('MUSTNOTPASS');
      // stopping the agent removed its worktree; the clone stays until the run is torn down
      await until(() => !existsSync(join(agentsRoot, id, agent.id)), 5000, 'the worktree to be removed');
      expect(existsSync(join(agentsRoot, id, 'clone', '.git'))).toBe(true);
      expect(engine.runActive()).toBe(false);

      // idle window -> torn down by the tick loop, with a receipt
      await until(() => ledger.getSandbox('ratchet-sandbox')!.status === 'destroyed', 20_000, 'the idle teardown');
      const receipt = (await engine.teardownReceipt(id))!;
      expect(receipt).toMatchObject({ sandboxId: 'ratchet-sandbox', trigger: 'idle', nothingRunning: true, runs: [{ optimizationId: id, status: 'completed' }] });
      expect(receipt.spendUsd).toBeCloseTo(0.06, 5); // two agent turns at the CLI's own cost report ($0.03 each)
      expect(existsSync(join(agentsRoot, id))).toBe(false); // teardown released the local clone
      expect(await sandboxProvider.outputs('ratchet-sandbox')).toBeNull();
      expect(events.filter((e) => e.type === 'run-active').map((e) => (e.type === 'run-active' ? e.active : null))).toEqual([true, false]);
      expect(events.filter((e) => e.type === 'sandbox').map((e) => (e.type === 'sandbox' ? e.phase : null))).toEqual(['creating', 'ready', 'idle', 'destroying', 'destroyed']);
    } finally {
      stop.abort();
      await loop;
      await engine.close();
      await localRunners.stopAll();
    }
  }, 120_000);
});
