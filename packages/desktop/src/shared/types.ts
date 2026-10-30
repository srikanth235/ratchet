import type { AgentKind, OptimizationStatus } from '@ratchet/ui';

export interface Kpi {
  id: string;
  label: string;
  value: string;
  unit?: string;
  delta: string;
  trend: number[];
}

/** A ratchet step: each verified improvement lowers (or raises) the best metric value. */
export interface RatchetPoint {
  t: number; // minutes since start
  value: number;
  agent: AgentKind;
  /** false = candidate that did not beat the ratchet */
  verified: boolean;
}

export interface Optimization {
  id: string;
  name: string;
  repo: string;
  metric: string;
  unit: string;
  direction: 'lower' | 'higher';
  baseline: number;
  best: number;
  status: OptimizationStatus;
  agents: AgentKind[];
  spendUsd: number;
  updated: string;
  history: RatchetPoint[];
}

export interface ActivityItem {
  id: string;
  agent: AgentKind | null;
  text: string;
  at: string;
  tone: 'brass' | 'mint' | 'rose' | 'neutral';
}

export interface OptimizationsSnapshot {
  kpis: Kpi[];
  featuredId: string;
  optimizations: Optimization[];
  activity: ActivityItem[];
}

/** The only surface the renderer gets from the main process. */
export interface RatchetApi {
  listOptimizations(): Promise<OptimizationsSnapshot>;
}

export const IPC = { listOptimizations: 'ratchet:list-optimizations' } as const;
