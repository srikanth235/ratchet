import { readFileSync } from 'node:fs';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Sparkline, sparklinePoints, StatusPill, tokens, tokensToCssVars } from './index.js';

describe('tokens', () => {
  it('matches the architecture doc palette', () => {
    expect(tokens.color.window).toBe('#0B1019');
    expect(tokens.color.brass).toBe('#D9B26F');
    expect(tokens.color.claudeCode).toBe('#B8A6FF');
    expect(tokens.color.codex).toBe('#7CC4F2');
    expect(tokens.radius.card).toBe('14px');
  });

  it('tokens.css declares every token with the same value', () => {
    const css = readFileSync(new URL('./tokens.css', import.meta.url), 'utf8');
    for (const [name, value] of Object.entries(tokensToCssVars())) {
      expect(css).toContain(`${name}: ${value};`);
    }
  });
});

describe('Sparkline', () => {
  it('maps min to bottom and max to top', () => {
    const pts = sparklinePoints([1, 3], 100, 20);
    expect(pts).toEqual(['2.0,18.0', '98.0,2.0']);
  });

  it('renders an svg polyline, and nothing for <2 points', () => {
    expect(renderToStaticMarkup(<Sparkline values={[1, 2, 3]} />)).toContain('<polyline');
    expect(renderToStaticMarkup(<Sparkline values={[1]} />)).not.toContain('<polyline');
  });
});

describe('StatusPill', () => {
  it('labels status', () => {
    expect(renderToStaticMarkup(<StatusPill status="pr-open" />)).toContain('PR open');
  });
});
