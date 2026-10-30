import type { ReactNode } from 'react';

export type PillTone = 'brass' | 'mint' | 'rose' | 'neutral' | 'quiet' | 'outline';

export interface PillProps {
  tone?: PillTone;
  dot?: boolean;
  live?: boolean;
  /** 26px header-size pill instead of the 24px status pill. */
  large?: boolean;
  children?: ReactNode;
}

export function Pill({ tone = 'neutral', dot = false, live = false, large = false, children }: PillProps) {
  const cls = ['rt-pill', `rt-pill--${tone}`, live && 'rt-pill--live', large && 'rt-pill--lg'].filter(Boolean).join(' ');
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
  draft: { tone: 'quiet', label: 'Draft' },
  blocked: { tone: 'rose', label: 'Blocked' },
};

export function StatusPill({ status, large = false, label }: { status: OptimizationStatus; large?: boolean; label?: string }) {
  const s = STATUS[status];
  return (
    <Pill tone={s.tone} dot live={s.live} large={large}>
      {label ?? s.label}
    </Pill>
  );
}

export type Verdict = 'pending' | 'verified' | 'rejected' | 'failed_confirm' | 'blocked';

const VERDICT: Record<Verdict, { tone: PillTone; label: string }> = {
  pending: { tone: 'neutral', label: 'Pending' },
  verified: { tone: 'mint', label: 'Verified win' },
  rejected: { tone: 'neutral', label: 'Rejected' },
  failed_confirm: { tone: 'outline', label: 'Failed confirm' },
  blocked: { tone: 'rose', label: 'Blocked' },
};

export function VerdictPill({ verdict }: { verdict: Verdict }) {
  const v = VERDICT[verdict];
  return <Pill tone={v.tone}>{v.label}</Pill>;
}
