import type { Optimization } from '../shared/types';

export function fmtMetric(v: number, unit: string): string {
  const n = unit === 's' ? v.toFixed(2) : Number.isInteger(v) ? String(v) : v.toFixed(1);
  return `${n} ${unit}`;
}

/** Signed % improvement; positive is always better regardless of direction. */
export function gainPct(o: Optimization): number | null {
  if (!o.baseline || !o.best) return null;
  const d = o.direction === 'lower' ? o.baseline - o.best : o.best - o.baseline;
  return (d / o.baseline) * 100;
}
