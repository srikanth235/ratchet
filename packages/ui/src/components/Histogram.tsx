import { useId } from 'react';
import { tokens } from '../tokens.js';
import { formatTick, histogramBins, linearScale } from './charts.js';

export interface HistogramSeries {
  name: string;
  values: readonly number[];
  color?: string;
}

export interface HistogramProps {
  /** Typically [baseline, best]; drawn as overlaid translucent bars. */
  series: readonly HistogramSeries[];
  bins?: number;
  unit?: string;
  width?: number;
  height?: number;
}

const M = { l: 12, r: 12, t: 12, b: 26 };
const DEFAULT_COLORS = [tokens.color.textTertiary, tokens.color.brass];

/** Overlaid histograms over a shared x range (e.g. latency samples, baseline vs best). */
export function Histogram({ series, bins = 14, unit = '', width = 420, height = 180 }: HistogramProps) {
  const uid = useId();
  const all = series.flatMap((s) => s.values).filter(Number.isFinite);
  if (all.length === 0) return null;
  const lo = Math.min(...all);
  const hi = Math.max(...all);
  const span = hi - lo || 1;
  const counts = series.map((s) => histogramBins(s.values, lo, lo + span, bins));
  const maxCount = Math.max(1, ...counts.flat());
  const x = linearScale([lo, lo + span], [M.l, width - M.r]);
  const y = linearScale([0, maxCount], [height - M.b, M.t]);
  const bw = (width - M.l - M.r) / bins;
  return (
    <div>
      <svg
        className="rt-chart"
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={`Histogram of ${series.map((s) => s.name).join(' vs ')}`}
      >
        <line x1={M.l} x2={width - M.r} y1={height - M.b} y2={height - M.b} stroke={tokens.color.hairline} />
        {series.map((s, si) =>
          counts[si]!.map((c, i) =>
            c === 0 ? null : (
              <rect
                key={`${uid}-${si}-${i}`}
                data-series={s.name}
                x={M.l + i * bw + 1}
                y={y(c)}
                width={Math.max(1, bw - 2)}
                height={height - M.b - y(c)}
                rx="2"
                fill={s.color ?? DEFAULT_COLORS[si % DEFAULT_COLORS.length]}
                opacity=".55"
              />
            ),
          ),
        )}
        <text x={M.l} y={height - 8} className="rt-chart__tick">{formatTick(lo)}{unit && ` ${unit}`}</text>
        <text x={width - M.r} y={height - 8} textAnchor="end" className="rt-chart__tick">{formatTick(lo + span)}</text>
      </svg>
      <div className="rt-legend">
        {series.map((s, si) => (
          <span key={s.name}><i style={{ background: s.color ?? DEFAULT_COLORS[si % DEFAULT_COLORS.length] }} />{s.name}</span>
        ))}
      </div>
    </div>
  );
}
