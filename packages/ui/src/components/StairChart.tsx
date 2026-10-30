import { useId } from 'react';
import { tokens } from '../tokens.js';
import { agentColor, type AgentKind } from './AgentAvatar.js';
import { formatTick, linearScale, ticks } from './charts.js';

export interface StairStep {
  /** Minutes (or any x unit) since start. */
  t: number;
  value: number;
}

export interface StairPoint extends StairStep {
  agent: AgentKind;
  /** false = candidate that did not move the ratchet (drawn hollow). */
  verified: boolean;
  label?: string;
}

export interface StairChartProps {
  /** Ratchet staircase: the best value over time. The first step is the baseline. */
  steps: readonly StairStep[];
  /** Candidate scatter. */
  points?: readonly StairPoint[];
  direction: 'lower' | 'higher';
  /** Baseline noise band, drawn as a horizontal band around the baseline value. */
  noiseBand?: { center: number; halfWidth: number } | undefined;
  /** Unit suffix used in the accessible label. */
  unit?: string;
  /** Right edge of the x axis; defaults to the last point. */
  tMax?: number;
  width?: number;
  height?: number;
  ariaLabel?: string;
}

const M = { l: 48, r: 16, t: 16, b: 28 };

/** Build the staircase polyline: hold the previous best until the next step lands. */
export function stairPath(steps: readonly StairStep[], tMax: number): [number, number][] {
  if (steps.length === 0) return [];
  const out: [number, number][] = [[steps[0]!.t, steps[0]!.value]];
  let best = steps[0]!.value;
  for (const s of steps.slice(1)) {
    out.push([s.t, best], [s.t, s.value]);
    best = s.value;
  }
  out.push([Math.max(tMax, steps.at(-1)!.t), best]);
  return out;
}

/** The ratchet chart: staircase of best-so-far, candidate dots, optional noise band, dot-grid backdrop. */
export function StairChart({
  steps,
  points = [],
  direction,
  noiseBand,
  unit = '',
  tMax: tMaxProp,
  width = 720,
  height = 260,
  ariaLabel,
}: StairChartProps) {
  const uid = useId().replace(/:/g, '');
  if (steps.length === 0) return null;

  const tMax = (tMaxProp ?? Math.max(...steps.map((s) => s.t), ...points.map((p) => p.t))) * 1.06 || 1;
  const vals = [...steps.map((s) => s.value), ...points.map((p) => p.value)];
  if (noiseBand) vals.push(noiseBand.center - noiseBand.halfWidth, noiseBand.center + noiseBand.halfWidth);
  const lo = Math.min(...vals);
  const hi = Math.max(...vals);
  const pad = (hi - lo) * 0.15 || 1;
  const vMin = lo - pad;
  const vMax = hi + pad;
  const x = linearScale([0, tMax], [M.l, width - M.r]);
  const y = linearScale([vMin, vMax], [height - M.b, M.t]);

  const path = stairPath(steps, tMax);
  const line = path.map(([t, v]) => `${x(t).toFixed(1)},${y(v).toFixed(1)}`).join(' ');
  const base = height - M.b;
  const area = `${x(path[0]![0]).toFixed(1)},${base} ${line} ${x(tMax).toFixed(1)},${base}`;
  const first = steps[0]!.value;
  const last = steps.at(-1)!.value;
  const label =
    ariaLabel ?? `Ratchet chart, best ${direction === 'lower' ? 'down' : 'up'} from ${formatTick(first)} to ${formatTick(last)} ${unit}`.trim();

  return (
    <svg className="rt-chart rt-stair" viewBox={`0 0 ${width} ${height}`} role="img" aria-label={label}>
      <defs>
        <linearGradient id={`${uid}-fill`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={tokens.color.brass} stopOpacity=".32" />
          <stop offset="100%" stopColor={tokens.color.brass} stopOpacity="0" />
        </linearGradient>
        <pattern id={`${uid}-grid`} width="16" height="16" patternUnits="userSpaceOnUse">
          <circle cx="1" cy="1" r="1" fill="rgba(148,163,190,.16)" />
        </pattern>
      </defs>
      <rect x={M.l} y={M.t} width={width - M.l - M.r} height={height - M.t - M.b} fill={`url(#${uid}-grid)`} />
      {ticks(vMin, vMax).map((v) => (
        <g key={v}>
          <line x1={M.l} x2={width - M.r} y1={y(v)} y2={y(v)} stroke={tokens.color.hairline} />
          <text x={M.l - 8} y={y(v) + 4} textAnchor="end" className="rt-chart__tick">{formatTick(v)}</text>
        </g>
      ))}
      {noiseBand && (
        <rect
          data-testid="noise-band"
          x={M.l}
          width={width - M.l - M.r}
          y={y(noiseBand.center + noiseBand.halfWidth)}
          height={Math.max(1, y(noiseBand.center - noiseBand.halfWidth) - y(noiseBand.center + noiseBand.halfWidth))}
          fill={tokens.color.textSecondary}
          opacity=".12"
        />
      )}
      <polygon points={area} fill={`url(#${uid}-fill)`} />
      <polyline data-testid="stair-line" points={line} fill="none" stroke={tokens.color.brass} strokeWidth="2" strokeLinejoin="round" />
      {points.map((p, i) => {
        const c = agentColor(p.agent);
        return (
          <circle
            key={`${i}-${p.t}-${p.value}`}
            data-verified={p.verified}
            cx={x(p.t)}
            cy={y(p.value)}
            r="4.5"
            fill={p.verified ? c : tokens.color.card}
            stroke={c}
            strokeWidth="2"
          >
            {p.label && <title>{p.label}</title>}
          </circle>
        );
      })}
      <text x={M.l} y={height - 8} className="rt-chart__tick">0h</text>
      <text x={width - M.r} y={height - 8} textAnchor="end" className="rt-chart__tick">{(tMax / 60).toFixed(1)}h</text>
    </svg>
  );
}
