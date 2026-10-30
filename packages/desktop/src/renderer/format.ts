import type { Optimization } from '../shared/types';

export function fmtNum(v: number): string {
  if (Math.abs(v) >= 100) return v.toFixed(0);
  if (v !== 0 && Math.abs(v) < 0.01) return String(Number(v.toPrecision(2))); // error rates: 0.0002, not 0.00
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
  return m % 60 === 0 ? `${h}h` : `${h}h ${m % 60}m`;
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

/** Short icon label for a metric key: `http_req_duration.p95` → `p95`, `events_per_s` → `eps`, `cold_start.s` → `cs`. */
export function metricGlyph(metric: string): string {
  const segs = metric.split('.').filter(Boolean);
  let pick = segs.at(-1) ?? metric;
  if (pick.length <= 2 && segs.length > 1) pick = segs.at(-2)!;
  if (/^[a-z]?\d+$/i.test(pick)) return pick.toLowerCase();
  const words = pick.split(/[_\-\s]+/).filter(Boolean);
  return (words.length > 1 ? words.map((w) => w[0]).join('').slice(0, 3) : pick.slice(0, 3)).toLowerCase();
}

/** `acme/search-api` for a GitHub URL; `local · remote.git` for a filesystem path (the simulation's repo). */
export function repoLabel(repo: string): string {
  const gh = /^(?:https?:\/\/|git@)github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?\/?$/.exec(repo);
  if (gh) return gh[1]!;
  if (repo.startsWith('/') || /^[A-Za-z]:[\\/]/.test(repo)) return `local · ${repo.split(/[\\/]/).filter(Boolean).at(-1) ?? repo}`;
  return repo;
}

/** `$1,284` / `$18.2k` / `$1.2M`. */
export function fmtUsdCompact(v: number): string {
  const a = Math.abs(v);
  if (a >= 1_000_000) return `$${(v / 1_000_000).toFixed(1)}M`;
  if (a >= 10_000) return `$${(v / 1000).toFixed(1)}k`;
  return `$${Math.round(v).toLocaleString('en-US')}`;
}

/** Return on spend: `12.4×`, `184×`, `18.2k×`. */
export const fmtRoi = (x: number): string =>
  x >= 1_000_000 ? `${(x / 1_000_000).toFixed(1)}M×` : x >= 10_000 ? `${(x / 1000).toFixed(1)}k×` : `${x >= 100 ? x.toFixed(0) : x.toFixed(1)}×`;

/** "Demo: search API latency" -> "Search API latency" (the demo section already says it is a demo). */
export function demoTitle(title: string): string {
  const t = title.replace(/^Demo:\s*/i, '');
  return t.charAt(0).toUpperCase() + t.slice(1);
}
