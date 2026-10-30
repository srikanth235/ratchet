import type { InstanceInfo } from '@ratchet/core';
import type { Deps } from './deps.js';
import { K } from './kv.js';

const ENDED = new Set(['stopped', 'completed', 'failed']);
const DEAD_AGENT = new Set(['stopped', 'done', 'failed']);

/**
 * Terminate tagged instances that should not exist: anything of an ended optimization, anything of
 * a stopped / finished agent, experiments and bench instances past their max lifetime, and agent
 * runners past the time limit plus a grace period. Returns the ids it asked to terminate.
 */
export async function reap(d: Deps, optimizationId: string, instances?: InstanceInfo[]): Promise<string[]> {
  const { ledger, clock, compute, settings, kv } = d;
  const opt = ledger.getOptimization(optimizationId);
  if (!opt) return [];
  const live = (instances ?? (await compute.listTagged(optimizationId))).filter(
    (i) => i.optimizationId === optimizationId && i.state !== 'terminated',
  );
  if (live.length === 0) return [];

  const now = clock.now();
  const agents = new Map(ledger.listAgents(optimizationId).map((a) => [a.id, a]));
  const deadline = kv.getNumber(K.deadline(optimizationId));
  const doomed: string[] = [];

  for (const i of live) {
    const age = now - i.launchedAt;
    const agent = i.agentId ? agents.get(i.agentId) : undefined;
    let kill = ENDED.has(opt.status);
    if (!kill && agent && DEAD_AGENT.has(agent.status)) kill = true;
    if (!kill && i.role === 'experiment' && age > settings.reaper.maxExperimentLifetimeMs) kill = true;
    if (!kill && i.role === 'bench' && age > settings.reaper.maxBenchLifetimeMs) kill = true;
    if (!kill && i.role === 'agent_runner') {
      const limit = (deadline ?? opt.createdAt + opt.timeLimitS * 1000) + settings.reaper.runnerGraceMs;
      if (now > limit) kill = true;
    }
    if (kill) doomed.push(i.id);
  }
  if (doomed.length > 0) await compute.terminate(doomed);
  return doomed;
}
