/** Shared helpers for the SVG charts. Pure, so they are unit-testable without a DOM. */

export interface Scale {
  (v: number): number;
  domain: [number, number];
}

export function linearScale(domain: [number, number], range: [number, number]): Scale {
  const [d0, d1] = domain;
  const span = d1 - d0 || 1;
  const fn = ((v: number) => range[0] + ((v - d0) / span) * (range[1] - range[0])) as Scale;
  fn.domain = domain;
  return fn;
}

/** `count` evenly spaced tick values across a domain. */
export function ticks(lo: number, hi: number, count = 5): number[] {
  if (count < 2) return [lo];
  return Array.from({ length: count }, (_, i) => lo + ((hi - lo) * i) / (count - 1));
}

/** Compact tick label: keeps 2 decimals for small magnitudes, none for big ones. */
export function formatTick(v: number): string {
  const a = Math.abs(v);
  if (a >= 100) return v.toFixed(0);
  if (a >= 10) return v.toFixed(1);
  return v.toFixed(2);
}

/** Bin `values` into `bins` equal-width buckets spanning [lo, hi]. */
export function histogramBins(values: readonly number[], lo: number, hi: number, bins: number): number[] {
  const out = new Array<number>(bins).fill(0);
  const span = hi - lo || 1;
  for (const v of values) {
    if (!Number.isFinite(v)) continue;
    const i = Math.min(bins - 1, Math.max(0, Math.floor(((v - lo) / span) * bins)));
    out[i]! += 1;
  }
  return out;
}
