import { tokens } from '../tokens.js';
import { formatTick, histogramBins, linearScale } from './charts.js';

export interface HistogramSeries {
  name: string;
  values: readonly number[];
  color?: string;
}

export interface HistogramProps {
  /** Typically [baseline, best]: the first is drawn as an outline, the rest as filled brass steps. */
  series: readonly HistogramSeries[];
  bins?: number;
  unit?: string;
  /** Dashed vertical guides, e.g. the p95 of each series. */
  markers?: readonly { value: number; label: string; dashed?: boolean }[];
  /** Draw the legend under the chart. */
  legend?: boolean;
  width?: number;
  height?: number;
}

const M = { l: 4, r: 4, t: 26, b: 26 };

/** Stepped outline of bin counts: M x0,base → bin tops → base. */
function steppedPath(counts: readonly number[], x: (v: number) => number, y: (v: number) => number, lo: number, span: number, base: number): string {
  const bins = counts.length;
  let d = `M${x(lo).toFixed(1)},${base}`;
  counts.forEach((c, i) => {
    const x0 = x(lo + (span * i) / bins);
    const x1 = x(lo + (span * (i + 1)) / bins);
    d += ` L${x0.toFixed(1)},${y(c).toFixed(1)} L${x1.toFixed(1)},${y(c).toFixed(1)}`;
  });
  return `${d} L${x(lo + span).toFixed(1)},${base}`;
}

/** Overlaid latency histograms over a shared x range (baseline outline vs best fill). */
export function Histogram({ series, bins: binsProp, unit = '', markers = [], legend = false, width = 560, height = 214 }: HistogramProps) {
  const all = series.flatMap((s) => s.values).filter(Number.isFinite);
  if (all.length === 0) return null;
  const bins = binsProp ?? Math.max(8, Math.min(28, Math.round(Math.sqrt(Math.max(...series.map((s) => s.values.length))) * 4)));
  const lo = Math.min(...all);
  const hi = Math.max(...all);
  const span = hi - lo || 1;
  const counts = series.map((s) => histogramBins(s.values, lo, lo + span, bins));
  const maxCount = Math.max(1, ...counts.flat());
  const base = height - M.b;
  const x = linearScale([lo, lo + span], [M.l, width - M.r]);
  const y = linearScale([0, maxCount], [base, M.t]);
  return (
    <div>
      <svg
        className="rt-chart"
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label={`Histogram of ${series.map((s) => s.name).join(' vs ')}`}
        style={{ width: '100%', height: 'auto', minWidth: 420 }}
      >
        <line x1={M.l} x2={width - M.r} y1={base} y2={base} stroke="rgba(148,163,190,.3)" strokeWidth="1.4" />
        {series.map((s, si) => {
          const d = steppedPath(counts[si]!, x, y, lo, span, base);
          const color = s.color ?? (si === 0 ? tokens.color.textTertiary : tokens.color.brass);
          return si === 0 ? (
            <path key={s.name} data-series={s.name} d={d} fill="none" stroke={color} strokeWidth="1.4" strokeLinejoin="round" />
          ) : (
            <path key={s.name} data-series={s.name} d={d} fill={color} fillOpacity=".55" stroke={color} strokeWidth="1.2" strokeLinejoin="round" />
          );
        })}
        {markers.map((m) => (
          <g key={m.label}>
            <line x1={x(m.value)} x2={x(m.value)} y1={M.t - 8} y2={base} stroke={m.dashed ? tokens.color.textSecondary : tokens.color.brass} strokeDasharray="3 4" opacity=".7" />
            <text x={x(m.value) + 4} y={M.t - 2} className="rt-chart__tick" style={{ fill: tokens.color.textSecondary }}>{m.label}</text>
          </g>
        ))}
        <text x={M.l} y={height - 8} className="rt-chart__tick">{formatTick(lo)}{unit && ` ${unit}`}</text>
        <text x={width - M.r} y={height - 8} textAnchor="end" className="rt-chart__tick">{formatTick(lo + span)}</text>
      </svg>
      {legend && (
        <div className="rt-legend">
          {series.map((s, si) => (
            <span key={s.name}><i style={{ width: 12, height: 10, borderRadius: 2, background: si === 0 ? 'transparent' : (s.color ?? tokens.color.brass), border: si === 0 ? `1.4px solid ${tokens.color.textTertiary}` : 0 }} />{s.name}</span>
          ))}
        </div>
      )}
    </div>
  );
}
