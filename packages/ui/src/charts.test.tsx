import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { BarSpark, barHeights, Histogram, histogramBins, linearScale, StairChart, stairPath, ticks, VerdictPill } from './index.js';

describe('chart helpers', () => {
  it('linearScale maps domain to range', () => {
    const s = linearScale([0, 10], [100, 200]);
    expect(s(0)).toBe(100);
    expect(s(10)).toBe(200);
    expect(s(5)).toBe(150);
  });

  it('ticks span the domain inclusively', () => {
    expect(ticks(0, 8, 5)).toEqual([0, 2, 4, 6, 8]);
  });

  it('histogramBins counts every in-range sample, clamping the max into the last bin', () => {
    const b = histogramBins([0, 1, 1, 9, 10], 0, 10, 5);
    expect(b).toEqual([3, 0, 0, 0, 2]);
    expect(b.reduce((a, c) => a + c, 0)).toBe(5);
  });

  it('barHeights keeps the smallest bar visible and the largest at full height', () => {
    const h = barHeights([1, 5], 100);
    expect(h[0]).toBeCloseTo(20);
    expect(h[1]).toBeCloseTo(100);
  });
});

describe('StairChart', () => {
  const steps = [
    { t: 0, value: 182 },
    { t: 30, value: 168 },
    { t: 90, value: 150 },
  ];

  it('stairPath holds the previous best until each step lands, then extends to tMax', () => {
    expect(stairPath(steps, 120)).toEqual([
      [0, 182], [30, 182], [30, 168], [90, 168], [90, 150], [120, 150],
    ]);
  });

  it('renders the line, hollow dots for unverified candidates, and the noise band', () => {
    const html = renderToStaticMarkup(
      <StairChart
        steps={steps}
        direction="lower"
        unit="ms"
        noiseBand={{ center: 182, halfWidth: 3 }}
        points={[
          { t: 30, value: 168, agent: 'claude-code', verified: true },
          { t: 50, value: 175, agent: 'codex', verified: false },
        ]}
      />,
    );
    expect(html).toContain('data-testid="stair-line"');
    expect(html).toContain('data-testid="noise-band"');
    expect(html).toContain('data-verified="false"');
    expect(html).toContain('data-verified="true"');
    expect(html).toContain('aria-label="Ratchet chart, best down from 182 to 150 ms"');
  });

  it('renders nothing without steps', () => {
    expect(renderToStaticMarkup(<StairChart steps={[]} direction="lower" />)).toBe('');
  });
});

describe('BarSpark / Histogram / VerdictPill', () => {
  it('BarSpark draws one rect per bar', () => {
    const html = renderToStaticMarkup(<BarSpark bars={[{ value: 1, tone: 'mint' }, { value: 3 }, { value: 2, tone: 'rose' }]} />);
    expect(html.match(/<rect/g)).toHaveLength(3);
  });

  it('Histogram overlays one series per input', () => {
    const html = renderToStaticMarkup(
      <Histogram series={[{ name: 'baseline', values: [1, 2, 3] }, { name: 'best', values: [1, 1, 2] }]} />,
    );
    expect(html).toContain('data-series="baseline"');
    expect(html).toContain('data-series="best"');
  });

  it('Histogram renders nothing without samples', () => {
    expect(renderToStaticMarkup(<Histogram series={[{ name: 'x', values: [] }]} />)).toBe('');
  });

  it('VerdictPill labels verdicts', () => {
    expect(renderToStaticMarkup(<VerdictPill verdict="failed_confirm" />)).toContain('Failed confirm');
  });
});
