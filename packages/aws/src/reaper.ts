import type { AgentId, CloudCompute, Clock, InstanceInfo, OptimizationId } from '@ratchet/core';
import { systemClock } from './util.js';

export type ReapReason = 'optimization-ended' | 'agent-stopped' | 'max-lifetime' | 'stopped-instance' | 'over-quota';

export interface ReapedInstance {
  instanceId: string;
  optimizationId: OptimizationId;
  agentId?: AgentId;
  role: InstanceInfo['role'];
  reason: ReapReason;
  ageMs: number;
}

export interface ReaperReport {
  at: number;
  scanned: number;
  terminated: ReapedInstance[];
  /** Agents that exceeded max_instances_per_agent (candidates for the kill switch on repeat offence). */
  overQuotaAgents: Array<{ optimizationId: OptimizationId; agentId: AgentId; running: number }>;
  errors: string[];
}

export interface ReaperOptions {
  compute: CloudCompute;
  clock?: Clock;
  /** `limits.max_instance_lifetime_hours` from the sandbox, in hours. */
  maxInstanceLifetimeHours: number;
  /** `limits.max_instances_per_agent`; omit to skip quota enforcement. */
  maxInstancesPerAgent?: number;
  isOptimizationEnded(id: OptimizationId): boolean | Promise<boolean>;
  isAgentStopped(optimizationId: OptimizationId, agentId: AgentId): boolean | Promise<boolean>;
}

const LIVE: ReadonlySet<InstanceInfo['state']> = new Set(['pending', 'running', 'stopped']);

/**
 * Terminates tagged instances that are past max lifetime, belong to a stopped
 * agent, or outlived their optimization. Stateless and idempotent: it reads the
 * cloud, terminates, and a second run finds nothing left.
 */
export class Reaper {
  private readonly clock: Clock;
  constructor(private readonly opts: ReaperOptions) {
    this.clock = opts.clock ?? systemClock;
  }

  async reap(optimizationId?: OptimizationId): Promise<ReaperReport> {
    const now = this.clock.now();
    const maxAge = this.opts.maxInstanceLifetimeHours * 3600_000;
    const all = await this.opts.compute.listTagged(optimizationId);
    // 'stopping'/'terminated' are already on their way out: skipping keeps reruns idempotent.
    const candidates = all.filter((i) => LIVE.has(i.state));
    const report: ReaperReport = { at: now, scanned: all.length, terminated: [], overQuotaAgents: [], errors: [] };

    const optEnded = new Map<string, boolean>();
    const agentStopped = new Map<string, boolean>();
    const doomed = new Map<string, ReapReason>();

    for (const i of candidates) {
      if (!optEnded.has(i.optimizationId)) optEnded.set(i.optimizationId, await this.opts.isOptimizationEnded(i.optimizationId));
      if (optEnded.get(i.optimizationId)) {
        doomed.set(i.id, 'optimization-ended');
        continue;
      }
      if (i.agentId) {
        const k = `${i.optimizationId}\0${i.agentId}`;
        if (!agentStopped.has(k)) agentStopped.set(k, await this.opts.isAgentStopped(i.optimizationId, i.agentId));
        if (agentStopped.get(k)) {
          doomed.set(i.id, 'agent-stopped');
          continue;
        }
      }
      if (now - i.launchedAt > maxAge) doomed.set(i.id, 'max-lifetime');
      else if (i.state === 'stopped') doomed.set(i.id, 'stopped-instance');
    }

    const quota = this.opts.maxInstancesPerAgent;
    if (quota !== undefined) {
      const byAgent = new Map<string, InstanceInfo[]>();
      for (const i of candidates) {
        if (i.role !== 'experiment' || !i.agentId || doomed.has(i.id) || i.state === 'stopped') continue;
        const k = `${i.optimizationId}\0${i.agentId}`;
        byAgent.set(k, [...(byAgent.get(k) ?? []), i]);
      }
      for (const list of byAgent.values()) {
        if (list.length <= quota) continue;
        const first = list[0]!;
        report.overQuotaAgents.push({ optimizationId: first.optimizationId, agentId: first.agentId!, running: list.length });
        // Keep the oldest `quota` instances, terminate the newest excess.
        for (const i of [...list].sort((a, b) => a.launchedAt - b.launchedAt).slice(quota)) doomed.set(i.id, 'over-quota');
      }
    }

    const targets = candidates.filter((i) => doomed.has(i.id));
    if (targets.length > 0) {
      try {
        await this.opts.compute.terminate(targets.map((i) => i.id));
        for (const i of targets) {
          report.terminated.push({
            instanceId: i.id,
            optimizationId: i.optimizationId,
            ...(i.agentId ? { agentId: i.agentId } : {}),
            role: i.role,
            reason: doomed.get(i.id)!,
            ageMs: now - i.launchedAt,
          });
        }
      } catch (e) {
        report.errors.push(`terminate failed: ${(e as Error).message}`);
      }
    }
    return report;
  }
}
