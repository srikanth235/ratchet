// The model Ratchet optimizes. TODO: replace the baseline with your forecaster.
// `history` is the series up to the forecast origin (never later periods); return `horizon` finite, non-negative numbers.

/** Baseline: the mean of the last 4 periods, repeated. */
export function forecast(history, horizon) {
  const recent = history.slice(-4);
  const level = recent.reduce((a, b) => a + b, 0) / Math.max(1, recent.length);
  return Array.from({ length: horizon }, () => level);
}
