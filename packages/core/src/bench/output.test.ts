import { describe, expect, it } from 'vitest';
import { extractLastJsonObject, extractMetric, lookupPath } from './output.js';

describe('extractLastJsonObject', () => {
  it('finds a lone object', () => {
    expect(extractLastJsonObject('{"a":1}')).toEqual({ a: 1 });
  });
  it('takes the last object when there are several', () => {
    expect(extractLastJsonObject('{"a":1}\nnoise\n{"a":2}\n')).toEqual({ a: 2 });
  });
  it('ignores surrounding text, including stray braces', () => {
    expect(extractLastJsonObject('starting {not json}\nrunning...\n{"x":{"y":[1,2,{"z":3}]}}\ndone')).toEqual({
      x: { y: [1, 2, { z: 3 }] },
    });
  });
  it('handles braces and escaped quotes inside strings', () => {
    expect(extractLastJsonObject('{"s":"a } \\" { b"}')).toEqual({ s: 'a } " { b' });
  });
  it('handles pretty-printed multi-line JSON', () => {
    expect(extractLastJsonObject('log\n{\n  "m": {\n    "p95": 12.5\n  }\n}\n')).toEqual({ m: { p95: 12.5 } });
  });
  it('returns undefined when there is none', () => {
    expect(extractLastJsonObject('')).toBeUndefined();
    expect(extractLastJsonObject('hello {')).toBeUndefined();
    expect(extractLastJsonObject('[1,2,3]')).toBeUndefined();
  });
});

describe('lookupPath', () => {
  it('walks dotted paths', () => {
    expect(lookupPath({ http_req_duration: { p95: 12 } }, 'http_req_duration.p95')).toBe(12);
    expect(lookupPath({ a: 1 }, 'a')).toBe(1);
  });
  it('supports keys containing dots and parens', () => {
    expect(lookupPath({ 'a.b': { c: 3 } }, 'a.b.c')).toBe(3);
    expect(lookupPath({ m: { values: { 'p(95)': 7 } } }, 'm.values.p(95)')).toBe(7);
  });
  it('returns undefined for missing paths and non-objects', () => {
    expect(lookupPath({ a: { b: 1 } }, 'a.c')).toBeUndefined();
    expect(lookupPath({ a: 1 }, 'a.b')).toBeUndefined();
    expect(lookupPath({}, 'toString')).toBeUndefined();
  });
});

describe('extractMetric', () => {
  it('returns the number', () => {
    expect(extractMetric('x\n{"lat":{"p95":42.5}}', 'lat.p95')).toEqual({ ok: true, value: 42.5 });
  });
  it('explains failures', () => {
    expect(extractMetric('nothing', 'lat.p95')).toEqual({ ok: false, error: 'no JSON object found on stdout' });
    expect(extractMetric('{"a":1}', 'lat.p95')).toMatchObject({ ok: false, error: expect.stringMatching(/not found/) });
    expect(extractMetric('{"lat":{"p95":"fast"}}', 'lat.p95')).toMatchObject({
      ok: false,
      error: expect.stringMatching(/not a finite number/),
    });
  });
});
