import { describe, expect, it } from 'vitest';
import { accuracy } from '../../eval/metrics.js';

describe('accuracy', () => {
  it('computes WAPE, MAPE and signed bias in percent', () => {
    const a = accuracy([[100, 0], [50, 50]], [[110, 5], [40, 60]]);
    expect(a.wape).toBeCloseTo((100 * (10 + 5 + 10 + 10)) / 200, 10);
    expect(a.bias).toBeCloseTo((100 * (10 + 5 - 10 + 10)) / 200, 10);
    expect(a.mape).toBeCloseTo((100 * (0.1 + 0.2 + 0.2)) / 3, 10); // a = 0 is skipped
  });

  it('is zero for a perfect forecast', () => {
    expect(accuracy([[3, 4]], [[3, 4]])).toEqual({ wape: 0, mape: 0, bias: 0 });
  });
});
