import type { Optimization } from '../shared/types';

export function fmtNum(v: number): string {
  if (Math.abs(v) >= 100) return v.toFixed(0);
  if (Math.abs(v) >= 10) return Number.isInteger(v) ? String(v) : v.toFixed(1);
  return Number.isInteger(v) ? String(v) : v.toFixed(2);
}

export function fmtMetric(v: number | null, unit: string): string {
  if (v === null) return '—';
  return unit ? `${fmtNum(v)} ${unit}` : fmtNum(v);
}

export function fmtUsd(v: number): string {
  return `$${v.toFixed(2)}`;
}

/** `3h 34m`, `12m`, `45s`. */
export function fmtDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

/** Signed % improvement; positive is always better regardless of direction. */
export function gainPct(o: Pick<Optimization, 'baseline' | 'best' | 'direction'>): number | null {
  if (!o.baseline || o.best === null) return null;
  const d = o.direction === 'lower' ? o.baseline - o.best : o.best - o.baseline;
  return (d / o.baseline) * 100;
}

export function fmtClock(at: number): string {
  const d = new Date(at);
  return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
}
