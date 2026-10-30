import type { ReactNode } from 'react';

export type PillTone = 'brass' | 'mint' | 'rose' | 'neutral';

export interface PillProps {
  tone?: PillTone;
  dot?: boolean;
  live?: boolean;
  children?: ReactNode;
}

export function Pill({ tone = 'neutral', dot = false, live = false, children }: PillProps) {
  const cls = ['rt-pill', `rt-pill--${tone}`, live && 'rt-pill--live'].filter(Boolean).join(' ');
  return (
    <span className={cls}>
      {dot && <span className="rt-pill__dot" aria-hidden="true" />}
      {children}
    </span>
  );
}

export type OptimizationStatus =
  | 'running'
  | 'baselining'
  | 'stopping'
  | 'completed'
  | 'stopped'
  | 'failed'
  | 'pr-open'
  | 'merged'
  | 'draft'
  | 'blocked';

const STATUS: Record<OptimizationStatus, { tone: PillTone; label: string; live?: boolean }> = {
  running: { tone: 'brass', label: 'Running', live: true },
  baselining: { tone: 'brass', label: 'Baselining', live: true },
  stopping: { tone: 'rose', label: 'Stopping', live: true },
  completed: { tone: 'mint', label: 'Completed' },
  stopped: { tone: 'neutral', label: 'Stopped' },
  failed: { tone: 'rose', label: 'Failed' },
  'pr-open': { tone: 'mint', label: 'PR open' },
  merged: { tone: 'mint', label: 'Merged' },
  draft: { tone: 'neutral', label: 'Draft' },
  blocked: { tone: 'rose', label: 'Blocked' },
};

export function StatusPill({ status }: { status: OptimizationStatus }) {
  const s = STATUS[status];
  return (
    <Pill tone={s.tone} dot live={s.live}>
      {s.label}
    </Pill>
  );
}

export type Verdict = 'pending' | 'verified' | 'rejected' | 'failed_confirm' | 'blocked';

const VERDICT: Record<Verdict, { tone: PillTone; label: string }> = {
  pending: { tone: 'neutral', label: 'Pending' },
  verified: { tone: 'mint', label: 'Verified' },
  rejected: { tone: 'neutral', label: 'Rejected' },
  failed_confirm: { tone: 'rose', label: 'Failed confirm' },
  blocked: { tone: 'rose', label: 'Blocked' },
};

export function VerdictPill({ verdict }: { verdict: Verdict }) {
  const v = VERDICT[verdict];
  return <Pill tone={v.tone}>{v.label}</Pill>;
}
