import { describe, expect, it } from 'vitest';
import { HORIZON, ORIGINS, backtestFolds, holdoutFolds, score } from '../../eval/backtest.js';
import { materializeHoldout } from '../../eval/holdout.js';
import { HOLDOUT_WEEKS, TRAIN_WEEKS, trainSales } from '../../data/generate.js';

describe('evaluator', () => {
  const train = trainSales();

  it('backtests rolling origins that stay inside the training period', () => {
    expect(HORIZON).toBe(HOLDOUT_WEEKS);
    const folds = backtestFolds(train);
    expect(folds).toHaveLength(ORIGINS.length);
    expect(Math.max(...ORIGINS) + HORIZON).toBeLessThanOrEqual(TRAIN_WEEKS);
    expect(folds[0]!.actual[0]).toEqual(train.units[0]!.slice(ORIGINS[0]!, ORIGINS[0]! + HORIZON));
  });

  it('the sealed holdout is the 12 weeks after training; its table carries the calendar but no sales', () => {
    const [fold] = holdoutFolds(train, materializeHoldout(7));
    expect(fold!.origin).toBe(TRAIN_WEEKS);
    expect(fold!.table.units[0]).toHaveLength(TRAIN_WEEKS);
    expect(fold!.table.promo[0]).toHaveLength(TRAIN_WEEKS + HOLDOUT_WEEKS);
    expect(fold!.table.holiday).toHaveLength(TRAIN_WEEKS + HOLDOUT_WEEKS);
    expect(fold!.actual[0]).toHaveLength(HOLDOUT_WEEKS);
  });

  it('reports wape, mape, bias and train_ms', () => {
    const r = score(backtestFolds(train).slice(0, 2));
    expect(Object.keys(r).sort()).toEqual(['bias', 'mape', 'train_ms', 'wape']);
    expect(r.wape).toBeGreaterThanOrEqual(0);
    expect(r.train_ms).toBeGreaterThanOrEqual(0);
  });
});
