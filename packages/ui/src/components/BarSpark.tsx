import { tokens } from '../tokens.js';

export interface Bar {
  value: number;
  /** Colour tone; defaults to neutral. */
  tone?: 'mint' | 'rose' | 'brass' | 'neutral';
  label?: string;
}

export interface BarSparkProps {
  bars: readonly Bar[];
  width?: number;
  height?: number;
  ariaLabel?: string;
}

const TONE: Record<NonNullable<Bar['tone']>, string> = {
  mint: tokens.color.mint,
  rose: tokens.color.rose,
  brass: tokens.color.brass,
  neutral: tokens.color.textTertiary,
};

/** Heights for each bar: scaled between 20% and 100% of the box so small values stay visible. */
export function barHeights(values: readonly number[], height: number): number[] {
  if (values.length === 0) return [];
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  const span = hi - lo;
  return values.map((v) => height * (span === 0 ? 0.6 : 0.2 + 0.8 * ((v - lo) / span)));
}

/** Tiny bar chart, e.g. an agent's recent eval results. */
export function BarSpark({ bars, width = 120, height = 32, ariaLabel = 'Recent evaluations' }: BarSparkProps) {
  if (bars.length === 0) return <svg width={width} height={height} role="img" aria-label={ariaLabel} />;
  const gap = 3;
  const bw = Math.max(2, (width - gap * (bars.length - 1)) / bars.length);
  const hs = barHeights(bars.map((b) => b.value), height);
  return (
    <svg className="rt-chart" width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label={ariaLabel}>
      {bars.map((b, i) => (
        <rect
          key={i}
          x={i * (bw + gap)}
          y={height - hs[i]!}
          width={bw}
          height={hs[i]}
          rx="2"
          fill={TONE[b.tone ?? 'neutral']}
          opacity={b.tone === 'neutral' || !b.tone ? 0.55 : 1}
        >
          {b.label && <title>{b.label}</title>}
        </rect>
      ))}
    </svg>
  );
}
