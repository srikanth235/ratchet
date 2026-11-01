/** Forecast accuracy metrics. `actual` and `forecast` are [sku][h] of the same shape. All percentages. */
export interface Accuracy {
  /** Weighted absolute percentage error: sum|a-f| / sum a, in percent. The objective. */
  wape: number;
  /** Mean absolute percentage error over points with a > 0, in percent. */
  mape: number;
  /** Signed bias: (sum f - sum a) / sum a, in percent. Positive = over-forecasting. */
  bias: number;
}

export function accuracy(actual: number[][], forecast: number[][]): Accuracy {
  let abs = 0;
  let total = 0;
  let signed = 0;
  let ape = 0;
  let apeN = 0;
  actual.forEach((row, s) => {
    row.forEach((a, h) => {
      const f = forecast[s]![h]!;
      abs += Math.abs(a - f);
      signed += f - a;
      total += a;
      if (a > 0) {
        ape += Math.abs(a - f) / a;
        apeN++;
      }
    });
  });
  return { wape: (100 * abs) / total, mape: (100 * ape) / Math.max(1, apeN), bias: (100 * signed) / total };
}
