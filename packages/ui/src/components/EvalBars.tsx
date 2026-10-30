import { tokens } from '../tokens.js';
import type { Bar } from './BarSpark.js';

const FILL: Record<NonNullable<Bar['tone']>, string> = {
  mint: tokens.color.mint,
  rose: tokens.color.rose,
  brass: tokens.color.brass,
  neutral: 'rgba(148,163,190,.32)',
};

export interface EvalBarsProps {
  bars: readonly (Bar & { verdict?: string; tint?: string })[];
  /** Value of the best verified result; drawn as a dashed brass guide across the bars. */
  best?: number | null;
  height?: number;
  ariaLabel?: string;
}

/** Heights between 6px and `height - 6`, scaled over the value range (blocked bars are a flat 6px stub). */
export function evalBarHeights(values: readonly number[], height: number): number[] {
  if (values.length === 0) return [];
  const lo = Math.min(...values);
  const hi = Math.max(...values);
  const span = hi - lo;
  return values.map((v) => Math.max(6, Math.round(span === 0 ? height * 0.6 : 6 + ((v - lo) / span) * (height - 12))));
}

/**
 * An agent's recent evaluations as flat bars: mint = verified, rose stub = blocked, dashed outline = failed confirm,
 * agent-tinted = rejected. A dashed brass guide marks the best verified value.
 */
export function EvalBars({ bars, best, height = 66, ariaLabel = 'Recent evaluations' }: EvalBarsProps) {
  const values = bars.map((b) => b.value);
  const hs = evalBarHeights(values, height - 16);
  const lo = values.length ? Math.min(...values) : 0;
  const hi = values.length ? Math.max(...values) : 1;
  const guide = best != null && hi !== lo ? 6 + ((best - lo) / (hi - lo)) * (height - 16 - 12) : null;
  return (
    <div className="rt-evalbars evalbars" role="img" aria-label={ariaLabel} style={{ height }}>
      {guide !== null && <span aria-hidden="true" className="rt-evalbars__best" style={{ bottom: Math.max(0, Math.min(height - 4, guide)) }} />}
      {bars.map((b, i) => {
        const tone = b.tone ?? 'neutral';
        const failed = b.verdict === 'failed_confirm';
        const blocked = b.verdict === 'blocked';
        return (
          <span
            key={i}
            title={b.label}
            data-verdict={b.verdict ?? tone}
            className="rt-evalbars__bar"
            style={{
              height: blocked ? 6 : hs[i],
              background: failed ? 'transparent' : tone === 'neutral' && b.tint ? b.tint : FILL[tone],
              border: failed ? `1px dashed ${tokens.color.textTertiary}` : 0,
            }}
          />
        );
      })}
    </div>
  );
}
