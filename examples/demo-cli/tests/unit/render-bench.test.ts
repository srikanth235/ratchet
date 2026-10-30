import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { generateMix } from '../../bench/gen-invocations.js';
import { quote, reduceExport } from '../../bench/hyperfine.js';
import { VISIBLE_MIX, percentile, summarize } from '../../bench/mix.js';
import { bar, renderTable } from '../../src/render/table.js';
import { csvField, renderCsv } from '../../src/render/csv.js';
import { escapeCell, escapeProse, renderMarkdownTable } from '../../src/render/markdown.js';

describe('render', () => {
  it('renderTable truncates the flexible column to the width', () => {
    const out = renderTable([{ title: 'N', align: 'right' }, { title: 'TEXT', flex: true }], [['1', 'a very long line of text that does not fit']], 20);
    const lines = out.trimEnd().split('\n');
    expect(lines[0]).toBe('N  TEXT');
    expect(lines[1]!.length).toBe(20);
    expect(lines[1]!.endsWith('…')).toBe(true);
  });

  it('bar draws unicode or ASCII', () => {
    expect(bar(5, 10, 10, true)).toBe('█████·····');
    expect(bar(5, 10, 10, false)).toBe('#####.....');
    expect(bar(0, 0, 4, false)).toBe('....');
  });

  it('csv quotes what needs it', () => {
    expect(csvField('plain')).toBe('plain');
    expect(csvField('a,b')).toBe('"a,b"');
    expect(csvField('say "hi"')).toBe('"say ""hi"""');
    expect(csvField(null)).toBe('');
    expect(renderCsv(['a', 'b'], [[1, 'x,y']])).toBe('a,b\n1,"x,y"\n');
  });

  it('markdown escapes pipes in cells and builds a table', () => {
    expect(escapeCell('a|b\\c')).toBe('a\\|b\\\\c');
    expect(renderMarkdownTable(['h'], [['v|1']])).toBe('| h |\n| --- |\n| v\\|1 |\n');
    expect(escapeProse('Tasks (3)')).toContain('Tasks');
  });
});

describe('bench', () => {
  it('percentile uses the nearest rank', () => {
    expect(percentile([5, 1, 3, 2, 4], 50)).toBe(3);
    expect(percentile([1, 2, 3, 4], 95)).toBe(4);
    expect(percentile([7], 0)).toBe(7);
  });

  it('summarize reports ms statistics and the error rate', () => {
    const s = summarize([10, 20, 30, 40], 1, 2);
    expect(s).toEqual({ cold_start_ms: { p50: 20, p95: 40, mean: 25, min: 10, max: 40 }, invocations: 2, runs: 4, error_rate: 0.25 });
  });

  it('reduceExport pools hyperfine runs (seconds) and counts unexpected exit codes', () => {
    const mix = { seed: 0, invocations: [{ args: ['list'] }, { args: ['nope'], expect: 2 }] };
    const exported = { results: [{ times: [0.1, 0.2], exit_codes: [0, 0] }, { times: [0.3, 0.4], exit_codes: [2, 1] }] };
    const r = reduceExport(exported, mix);
    expect(r.cold_start_ms.p50).toBe(200);
    expect(r.cold_start_ms.mean).toBe(250);
    expect(r.runs).toBe(4);
    expect(r.error_rate).toBe(0.25);
  });

  it('quote leaves safe words alone and single-quotes the rest', () => {
    expect(quote('/usr/bin/node')).toBe('/usr/bin/node');
    expect(quote('--limit')).toBe('--limit');
    expect(quote("it's here")).toBe(`'it'\\''s here'`);
  });

  it('the committed visible mix is what seed 1 generates', () => {
    expect(JSON.parse(readFileSync(VISIBLE_MIX, 'utf8'))).toEqual(generateMix(1));
  });

  it('other seeds draw other invocations of the same kinds', () => {
    const visible = generateMix(1).invocations;
    for (const seed of [2, 99, 424242]) {
      const hidden = generateMix(seed).invocations;
      expect(hidden).toHaveLength(visible.length);
      expect(hidden.slice(0, 10).map((i) => i.args[0])).toEqual(visible.slice(0, 10).map((i) => i.args[0]));
      expect(hidden.at(-1)!.expect).toBe(2);
      const seen = new Set(visible.map((i) => i.args.join(' ')));
      expect(hidden.filter((i) => seen.has(i.args.join(' '))).length).toBeLessThanOrEqual(1);
    }
  });
});
