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

/** `−28%` / `+43%`: signed gain with the metric's direction (a lower-is-better win shows a minus). */
export function gainLabel(o: Pick<Optimization, 'baseline' | 'best' | 'direction'>, digits = 0): string | null {
  const g = gainPct(o);
  if (g === null) return null;
  const sign = g === 0 ? '' : o.direction === 'lower' ? (g > 0 ? '−' : '+') : g > 0 ? '+' : '−';
  return `${sign}${Math.abs(g).toFixed(digits)}%`;
}

/** Short icon label for a metric key: `http_req_duration.p95` → `p95`, `events_per_s` → `eps`. */
export function metricGlyph(metric: string): string {
  const parts = metric.split(/[._\-\s]+/).filter(Boolean);
  const last = parts.at(-1) ?? metric;
  if (last.length <= 3 && (parts.length === 1 || /\d/.test(last))) return last.toLowerCase();
  const initials = parts.map((p) => p[0]).join('').toLowerCase();
  return initials.length >= 2 ? initials.slice(0, 3) : last.slice(0, 3).toLowerCase();
}
