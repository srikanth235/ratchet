import { useId, useState } from 'react';
import { tokens } from '../tokens.js';
import { linearScale, niceTicks } from './charts.js';

export interface ValueStep {
  /** Epoch ms of the win. */
  at: number;
  /** Cumulative value after this win (USD per year). */
  value: number;
  /** Named in the tooltip. */
  label: string;
}

export interface ValueStairProps {
  steps: readonly ValueStep[];
  /** The window the x axis spans, epoch ms. */
  from: number;
  to: number;
  width?: number;
  height?: number;
  ariaLabel?: string;
}

/** `$0`, `$850`, `$4.8k`, `$15.4k`, `$1.2M`: compact dollars for axes and tooltips. */
export function usdShort(v: number, digits = 1): string {
  const a = Math.abs(v);
  const trim = (n: number) => String(+n.toFixed(digits));
  if (a >= 1_000_000) return `$${trim(v / 1_000_000)}M`;
  if (a >= 1000) return `$${trim(v / 1000)}k`;
  return `$${Math.round(v)}`;
}

const L = 8;
const R = 58;
const T = 12;
const B = 28;
const dateLabel = (ms: number) => new Date(ms).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });

/** Step path: starts at $0 on the left edge, holds the previous value until the next win lands, runs to the right edge. */
export function valueStairPath(steps: readonly ValueStep[], x: (at: number) => number, y: (v: number) => number, xStart: number, xEnd: number): string {
  let d = `M${xStart.toFixed(1)} ${y(0).toFixed(1)}`;
  for (const s of steps) d += ` H${x(s.at).toFixed(1)} V${y(s.value).toFixed(1)}`;
  return `${d} H${xEnd.toFixed(1)}`;
}

/**
 * Value locked in over time (calm design): one brass staircase, hairline grid, the dollar axis on the right, a few date
 * ticks, one dot per verified win. Hovering (or focusing) a dot names the optimization.
 */
export function ValueStair({ steps, from, to, width = 760, height = 250, ariaLabel }: ValueStairProps) {
  const [active, setActive] = useState<number | null>(null);
  const uid = useId().replace(/:/g, '');
  const top = Math.max(0, ...steps.map((s) => s.value));
  const grid = top > 0 ? niceTicks(0, top * 1.04, 4) : [0, 1000];
  const vMax = grid.at(-1)!;
  const plotR = width - R;
  const span = to - from || 1;
  const x = (at: number) => L + ((Math.min(Math.max(at, from), to) - from) / span) * (plotR - L);
  const y = linearScale([0, vMax], [height - B, T]);
  const line = valueStairPath(steps, x, y, x(from), plotR);
  const area = `${line} V${y(0)} H${x(from)}Z`;
  const dateTicks = [0, 1 / 3, 2 / 3, 1].map((f, i) => ({ at: from + span * f, anchor: i === 0 ? 'start' : i === 3 ? 'end' : 'middle' }) as const);
  const label = ariaLabel ?? `Value locked in per year, ${steps.length === 0 ? 'no verified wins yet' : `rose to ${usdShort(top)} through ${steps.length} verified win${steps.length === 1 ? '' : 's'}`}`;
  const tip = active === null ? null : steps[active]!;
  const tipLeft = tip ? Math.min(92, Math.max(8, (x(tip.at) / width) * 100)) : 0;

  return (
    <div className="rt-valuestair" data-testid="value-stair">
      <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={label} className="rt-valuestair__svg">
        <defs>
          <linearGradient id={`vs-${uid}`} x1="0" x2="0" y1="0" y2="1">
            <stop offset="0" stopColor={tokens.color.brass} stopOpacity=".16" />
            <stop offset="1" stopColor={tokens.color.brass} stopOpacity="0" />
          </linearGradient>
        </defs>
        {grid.map((v) => (
          <g key={v}>
            <line x1={L} x2={plotR} y1={y(v)} y2={y(v)} stroke="rgba(148,163,190,.1)" />
            <text x={plotR + 10} y={y(v) + 4} className="rt-valuestair__tick">{usdShort(v, 0)}</text>
          </g>
        ))}
        <path d={area} fill={`url(#vs-${uid})`} />
        <path d={line} fill="none" stroke={tokens.color.brass} strokeWidth="2" strokeLinejoin="round" />
        {steps.map((s, i) => (
          <g key={`${s.at}-${i}`}>
            <circle cx={x(s.at)} cy={y(s.value)} r="4" fill={tokens.color.brass} stroke={tokens.color.window} strokeWidth="2" />
            <circle
              cx={x(s.at)}
              cy={y(s.value)}
              r="14"
              fill="transparent"
              tabIndex={0}
              role="img"
              aria-label={`${usdShort(s.value)} a year after ${s.label}`}
              data-win={i}
              style={{ cursor: 'pointer', outline: 'none' }}
              onMouseEnter={() => setActive(i)}
              onMouseLeave={() => setActive(null)}
              onFocus={() => setActive(i)}
              onBlur={() => setActive(null)}
            />
          </g>
        ))}
        {dateTicks.map((t, i) => (
          <text key={i} x={i === 3 ? plotR : x(t.at)} y={height - 6} textAnchor={t.anchor} className="rt-valuestair__date">{dateLabel(t.at)}</text>
        ))}
      </svg>
      {tip && (
        <div className="rt-valuestair__tip" role="tooltip" style={{ left: `${tipLeft}%`, top: `${(y(tip.value) / height) * 100}%` }}>
          <b>{usdShort(tip.value)} / yr</b> <span>· {tip.label}</span>
        </div>
      )}
    </div>
  );
}
