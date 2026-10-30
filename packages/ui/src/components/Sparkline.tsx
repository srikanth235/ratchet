import { tokens } from '../tokens.js';

export interface SparklineProps {
  values: readonly number[];
  width?: number;
  height?: number;
  color?: string;
  /** Fill the area under the line with a faint gradient. */
  fill?: boolean;
}

/** Map values to SVG points, padded by 2px so the stroke isn't clipped. */
export function sparklinePoints(values: readonly number[], width: number, height: number): string[] {
  if (values.length === 0) return [];
  const pad = 2;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  const step = values.length > 1 ? (width - pad * 2) / (values.length - 1) : 0;
  return values.map((v, i) => {
    const x = pad + i * step;
    const y = height - pad - ((v - min) / span) * (height - pad * 2);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
}

let gradientSeq = 0;

export function Sparkline({
  values,
  width = 96,
  height = 28,
  color = tokens.color.brass,
  fill = true,
}: SparklineProps) {
  const pts = sparklinePoints(values, width, height);
  const gid = `rt-spark-${++gradientSeq}`;
  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label="Trend">
      {pts.length > 1 && (
        <>
          {fill && (
            <>
              <defs>
                <linearGradient id={gid} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor={color} stopOpacity=".28" />
                  <stop offset="100%" stopColor={color} stopOpacity="0" />
                </linearGradient>
              </defs>
              <polygon
                points={`${pts.join(' ')} ${width - 2},${height} 2,${height}`}
                fill={`url(#${gid})`}
              />
            </>
          )}
          <polyline
            points={pts.join(' ')}
            fill="none"
            stroke={color}
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </>
      )}
    </svg>
  );
}
