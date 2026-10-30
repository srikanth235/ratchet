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

export type OptimizationStatus = 'running' | 'pr-open' | 'merged' | 'draft' | 'blocked';

const STATUS: Record<OptimizationStatus, { tone: PillTone; label: string; live?: boolean }> = {
  running: { tone: 'brass', label: 'Running', live: true },
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
