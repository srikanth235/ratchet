import { tokens } from '../tokens.js';
import { agentColor, type AgentKind } from './AgentAvatar.js';
import { formatTick, linearScale, niceTicks } from './charts.js';

export interface StairStep {
  /** Minutes (or any x unit) since start. */
  t: number;
  value: number;
}

export interface StairPoint extends StairStep {
  agent: AgentKind;
  /** false = candidate that did not move the ratchet. */
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
  /** Unit suffix, shown on the top-most axis tick and in the accessible label. */
  unit?: string;
  /** Elapsed time ("now"), in x units; defaults to the last step/point. */
  tMax?: number;
  /** Right edge of the x axis (e.g. the time limit, in minutes). When set, a "now" marker is drawn. */
  limit?: number;
  /** Annotate each ratchet lock with `value · message`. */
  annotate?: boolean;
  /** Label the baseline value with a dotted guide (benchmark-history variant). */
  baselineLabel?: boolean;
  /** Draw the dot-grid backdrop behind the plot. Default true. */
  dots?: boolean;
  width?: number;
  height?: number;
  ariaLabel?: string;
}

const PAD = { t: 18, b: 30 };

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

const hourLabel = (minutes: number) => `${+(minutes / 60).toFixed(1)}h`;
const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

/**
 * The ratchet chart (Midnight Brass): brass staircase of best-so-far on a dot grid, candidate dots in agent colours,
 * a haloed marker on every verified lock, optional "now" guide and noise band. Right-aligned mono axis ticks.
 */
export function StairChart({
  steps,
  points = [],
  direction,
  noiseBand,
  unit = '',
  tMax: tMaxProp,
  limit,
  annotate = true,
  baselineLabel = false,
  dots = true,
  width = 760,
  height = 240,
  ariaLabel,
}: StairChartProps) {
  if (steps.length === 0) return null;

  const now = Math.max(tMaxProp ?? 0, ...steps.map((s) => s.t), ...points.map((p) => p.t)) || 1;
  const axisEnd = limit && limit > now ? limit : now * 1.06;
  const vals = [...steps.map((s) => s.value), ...points.map((p) => p.value)];
  if (noiseBand) vals.push(noiseBand.center - noiseBand.halfWidth, noiseBand.center + noiseBand.halfWidth);
  const lo = Math.min(...vals);
  const hi = Math.max(...vals);
  const pad = (hi - lo) * 0.18 || 1;
  const grid = niceTicks(lo - pad, hi + pad, 4);
  const vMin = grid[0]!;
  const vMax = grid.at(-1)!;
  const base = height - PAD.b;
  const x = linearScale([0, axisEnd], [0, width]);
  const y = linearScale([vMin, vMax], [base, PAD.t]);

  const path = stairPath(steps, now);
  const f = (n: number) => n.toFixed(1);
  const d = path.map(([t, v], i) => `${i === 0 ? 'M' : 'L'}${f(x(t))},${f(y(v))}`).join(' ');
  const area = `${d} L${f(x(path.at(-1)![0]))},${base} L${f(x(path[0]![0]))},${base} Z`;
  const first = steps[0]!.value;
  const last = steps.at(-1)!.value;
  const best = last;
  const gradId = `rt-stair-fill-${Math.round(width)}-${Math.round(height)}`;
  const label =
    ariaLabel ?? `Ratchet chart, best ${direction === 'lower' ? 'down' : 'up'} from ${formatTick(first)} to ${formatTick(last)} ${unit}`.trim();

  const wins = steps.slice(1).map((s) => {
    const p = points.find((q) => q.verified && Math.abs(q.t - s.t) < 0.5 && Math.abs(q.value - s.value) < 1e-6);
    return { ...s, agent: p?.agent, label: p?.label };
  });

  const body = (
    <svg
      className="rt-chart rt-stair"
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={label}
      style={{ minWidth: Math.min(width, 520) }}
    >
      <defs>
        <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor={tokens.color.brass} stopOpacity=".30" />
          <stop offset="1" stopColor={tokens.color.brass} stopOpacity="0" />
        </linearGradient>
      </defs>
      {grid.map((v, i) => (
        <g key={v}>
          <line x1="0" x2={width} y1={y(v)} y2={y(v)} stroke={i === 0 ? 'rgba(148,163,190,.18)' : 'rgba(148,163,190,.10)'} />
          {i > 0 && (
            <text x={width - 4} y={y(v) - 4} textAnchor="end" className="rt-chart__tick">
              {formatTick(v)}
              {i === grid.length - 1 && unit ? ` ${unit}` : ''}
            </text>
          )}
        </g>
      ))}
      {noiseBand && (
        <rect
          data-testid="noise-band"
          x="0"
          width={width}
          y={y(noiseBand.center + noiseBand.halfWidth)}
          height={Math.max(1, y(noiseBand.center - noiseBand.halfWidth) - y(noiseBand.center + noiseBand.halfWidth))}
          fill={tokens.color.brass}
          opacity=".14"
        />
      )}
      {baselineLabel && (
        <>
          <line x1="0" x2={width} y1={y(first)} y2={y(first)} stroke="rgba(148,163,190,.5)" strokeDasharray="2 5" />
          <text x={x(now) + 6} y={y(first) - 6} className="rt-chart__tick" style={{ fill: tokens.color.textSecondary }}>
            baseline {formatTick(first)}
          </text>
        </>
      )}
      <path d={area} fill={`url(#${gradId})`} />
      <polyline
        data-testid="stair-line"
        points={path.map(([t, v]) => `${f(x(t))},${f(y(v))}`).join(' ')}
        fill="none"
        stroke={tokens.color.brass}
        strokeWidth="2.4"
        strokeLinejoin="round"
      />
      {x(now) < width - 8 && (
        <line x1={x(now)} x2={width - 40} y1={y(best)} y2={y(best)} stroke={tokens.color.brass} strokeWidth="1.2" strokeDasharray="2 6" opacity=".6" />
      )}
      {limit && limit > now && (
        <>
          <line x1={x(now)} x2={x(now)} y1={PAD.t} y2={base} stroke={tokens.color.text} strokeDasharray="3 4" opacity=".35" />
          <text x={x(now) + 6} y={PAD.t + 10} className="rt-chart__tick" style={{ fill: tokens.color.textSecondary }}>
            now · {hourLabel(now)}
          </text>
        </>
      )}
      {points
        .filter((p) => !p.verified)
        .map((p, i) => (
          <circle key={`${i}-${p.t}-${p.value}`} data-verified="false" cx={x(p.t)} cy={y(p.value)} r="3.5" fill={agentColor(p.agent)} opacity=".55">
            {p.label && <title>{p.label}</title>}
          </circle>
        ))}
      {wins.map((w, i) => (
        <g key={`win-${i}`} data-testid="lock">
          <circle cx={x(w.t)} cy={y(w.value)} r="9" fill="rgba(217,178,111,.18)" />
          <circle data-verified="true" cx={x(w.t)} cy={y(w.value)} r="5" fill={w.agent ? agentColor(w.agent) : tokens.color.brass} stroke={tokens.color.window} strokeWidth="2">
            {w.label && <title>{w.label}</title>}
          </circle>
          {annotate && (
            <text x={x(w.t) + 8} y={y(w.value) - 6} className="rt-chart__label">
              {formatTick(w.value)}
              {w.label ? ` · ${clip(w.label, 24)}` : ''}
            </text>
          )}
        </g>
      ))}
      <text x="0" y={height - 8} className="rt-chart__tick">0h</text>
      {[0.25, 0.5, 0.75].map((q) => (
        <text key={q} x={x(axisEnd * q)} y={height - 8} textAnchor="middle" className="rt-chart__tick">{hourLabel(axisEnd * q)}</text>
      ))}
      <text x={width - 4} y={height - 8} textAnchor="end" className="rt-chart__tick">{hourLabel(axisEnd)}{limit ? ' limit' : ''}</text>
    </svg>
  );
  return dots ? <div className="rt-dots">{body}</div> : body;
}
