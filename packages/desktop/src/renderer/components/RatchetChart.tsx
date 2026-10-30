import { tokens } from '@ratchet/ui';
import type { Optimization } from '../../shared/types';

const W = 720, H = 260;
const M = { l: 48, r: 16, t: 16, b: 28 };

/** Step ("ratchet") chart of the best metric value over time. Candidates that didn't beat it are hollow dots. */
export function RatchetChart({ opt }: { opt: Optimization }) {
  const pts = opt.history;
  if (pts.length === 0) return null;
  const tMax = Math.max(...pts.map((p) => p.t)) * 1.08 || 1;
  const vals = pts.map((p) => p.value);
  const lo = Math.min(...vals), hi = Math.max(...vals);
  const pad = (hi - lo) * 0.15 || 1;
  const vMin = lo - pad, vMax = hi + pad;
  const x = (t: number) => M.l + (t / tMax) * (W - M.l - M.r);
  const y = (v: number) => M.t + ((vMax - v) / (vMax - vMin)) * (H - M.t - M.b);
  const better = (a: number, b: number) => (opt.direction === 'lower' ? a < b : a > b);

  // Running best as a staircase: hold the previous best until a verified improvement lands.
  let best = pts[0]!.value;
  const stairs: [number, number][] = [[pts[0]!.t, best]];
  for (const p of pts.slice(1)) {
    if (p.verified && better(p.value, best)) {
      stairs.push([p.t, best], [p.t, p.value]);
      best = p.value;
    }
  }
  stairs.push([tMax, best]);
  const line = stairs.map(([t, v]) => `${x(t).toFixed(1)},${y(v).toFixed(1)}`).join(' ');
  const base = H - M.b;
  const area = `${x(stairs[0]![0])},${base} ${line} ${x(tMax)},${base}`;
  const ticks = Array.from({ length: 5 }, (_, i) => vMin + ((vMax - vMin) * i) / 4);

  return (
    <svg className="ratchet-chart" viewBox={`0 0 ${W} ${H}`} role="img"
      aria-label={`${opt.metric} ratchet from ${opt.baseline} to ${opt.best} ${opt.unit}`}>
      <defs>
        <linearGradient id="ratchet-fill" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={tokens.color.brass} stopOpacity=".32" />
          <stop offset="100%" stopColor={tokens.color.brass} stopOpacity="0" />
        </linearGradient>
        <pattern id="dotgrid" width="16" height="16" patternUnits="userSpaceOnUse">
          <circle cx="1" cy="1" r="1" fill="rgba(148,163,190,.16)" />
        </pattern>
      </defs>
      <rect x={M.l} y={M.t} width={W - M.l - M.r} height={H - M.t - M.b} fill="url(#dotgrid)" />
      {ticks.map((v) => (
        <g key={v}>
          <line x1={M.l} x2={W - M.r} y1={y(v)} y2={y(v)} stroke={tokens.color.hairline} />
          <text x={M.l - 8} y={y(v) + 4} textAnchor="end" className="chart-tick">{v.toFixed(opt.unit === 's' ? 2 : 0)}</text>
        </g>
      ))}
      <polygon points={area} fill="url(#ratchet-fill)" />
      <polyline points={line} fill="none" stroke={tokens.color.brass} strokeWidth="2" strokeLinejoin="round" />
      {pts.map((p) => {
        const c = p.agent === 'claude-code' ? tokens.color.claudeCode : tokens.color.codex;
        return (
          <circle key={`${p.t}-${p.value}`} cx={x(p.t)} cy={y(p.value)} r="4.5"
            fill={p.verified ? c : tokens.color.card} stroke={c} strokeWidth="2" />
        );
      })}
      <text x={M.l} y={H - 8} className="chart-tick">0h</text>
      <text x={W - M.r} y={H - 8} textAnchor="end" className="chart-tick">{(tMax / 60).toFixed(1)}h</text>
    </svg>
  );
}
