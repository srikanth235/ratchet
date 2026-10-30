import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { compileMessage } from '../../src/icu.js';
import { hasMessage, t } from '../../src/messages.js';
import { MESSAGES_DIR } from '../../src/paths.js';

describe('compileMessage', () => {
  it('passes plain text through and fills arguments', () => {
    expect(compileMessage('hello')()).toBe('hello');
    expect(compileMessage('{a} and {b}')({ a: 1, b: 'two' })).toBe('1 and two');
    expect(compileMessage('keeps {missing}')()).toBe('keeps {missing}');
  });

  it('plural: one / other / exact matches and #', () => {
    const m = compileMessage('{n, plural, =0 {nothing} one {# task} other {# tasks}}');
    expect(m({ n: 0 })).toBe('nothing');
    expect(m({ n: 1 })).toBe('1 task');
    expect(m({ n: 7 })).toBe('7 tasks');
  });

  it('select picks a case or falls back to other', () => {
    const m = compileMessage('{kind, select, a {Alpha} other {Other}}!');
    expect(m({ kind: 'a' })).toBe('Alpha!');
    expect(m({ kind: 'z' })).toBe('Other!');
  });

  it('nests arguments in cases', () => {
    expect(compileMessage('{n, plural, one {{who} has # item} other {{who} has # items}}')({ n: 3, who: 'Ann' })).toBe('Ann has 3 items');
  });

  it.each(['{', '}', '{n, plural, one {x}}', '{n, bogus, other {x}}', '{n, plural other {x}}'])('rejects %j', (src) => {
    expect(() => compileMessage(src)).toThrow(SyntaxError);
  });
});

describe('message catalogs', () => {
  it('formats the keys the commands use', () => {
    expect(t('list.summary', { shown: 5, total: 1 })).toBe('5 of 1 task');
    expect(t('errors.E0201', { id: 9 })).toBe('No task with id 9.');
    expect(t('not.a.key')).toBe('[not.a.key]');
    expect(hasMessage('help.title')).toBe(true);
  });

  it('every message of every catalog compiles, and keys are namespaced by the file name', () => {
    for (const file of readdirSync(MESSAGES_DIR).filter((f) => f.endsWith('.json'))) {
      const entries = JSON.parse(readFileSync(join(MESSAGES_DIR, file), 'utf8')) as Record<string, string>;
      for (const [key, source] of Object.entries(entries)) {
        expect(key.startsWith(`${file.slice(0, -5)}.`), `${file}: ${key}`).toBe(true);
        expect(() => compileMessage(source), `${file}: ${key}`).not.toThrow();
      }
    }
  });
});
