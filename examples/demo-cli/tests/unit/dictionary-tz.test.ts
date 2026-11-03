import { describe, expect, it } from 'vitest';
import { COMMAND_NAMES, VOCABULARY, editDistance, suggest } from '../../src/dictionary.js';
import { formatOffset, isKnownZone, localDate, zoneInfo } from '../../src/tz.js';

describe('dictionary', () => {
  it('is a large, sorted, duplicate-free vocabulary that contains the commands', () => {
    expect(VOCABULARY.length).toBeGreaterThanOrEqual(5000);
    expect(new Set(VOCABULARY).size).toBe(VOCABULARY.length);
    expect([...VOCABULARY].sort()).toEqual(VOCABULARY);
    for (const c of COMMAND_NAMES) expect(VOCABULARY).toContain(c);
  });

  it('editDistance counts insertions, deletions, substitutions and adjacent swaps', () => {
    expect(editDistance('list', 'list')).toBe(0);
    expect(editDistance('lst', 'list')).toBe(1);
    expect(editDistance('lsit', 'list')).toBe(1);
    expect(editDistance('lixt', 'list')).toBe(1);
    expect(editDistance('abcdefg', 'z', 2)).toBe(3); // gave up: more than max
  });

  it('suggests commands within two edits, nearest first', () => {
    expect(suggest('lst', 3, COMMAND_NAMES)).toContain('list');
    expect(suggest('serch', 3, COMMAND_NAMES)).toEqual(['search']);
    expect(suggest('exprot', 3, COMMAND_NAMES)).toEqual(['export']);
    expect(suggest('frobnicate', 3, COMMAND_NAMES)).toEqual([]);
  });

  it('suggests vocabulary words when not restricted', () => {
    const word = VOCABULARY[1234]!;
    const typo = word.slice(0, -1);
    expect(suggest(typo, 50)).toContain(word);
    expect(suggest(word)).not.toContain(word);
  });
});

describe('time zones', () => {
  it('knows IANA names and UTC, and nothing else', () => {
    expect(isKnownZone('Europe/Berlin')).toBe(true);
    expect(isKnownZone('UTC')).toBe(true);
    expect(isKnownZone('Mars/Olympus')).toBe(false);
    expect(() => zoneInfo('Mars/Olympus')).toThrow(RangeError);
  });

  it('reports standard and daylight offsets', () => {
    expect(zoneInfo('Europe/Berlin')).toEqual({ name: 'Europe/Berlin', standard: 60, daylight: 120 });
    expect(zoneInfo('Asia/Kolkata')).toEqual({ name: 'Asia/Kolkata', standard: 330, daylight: null });
    expect(zoneInfo('America/New_York')).toMatchObject({ standard: -300, daylight: -240 });
    expect(zoneInfo('Australia/Sydney')).toMatchObject({ standard: 600, daylight: 660 });
    expect(zoneInfo('UTC')).toMatchObject({ standard: 0, daylight: null });
  });

  it('formats offsets and local dates', () => {
    expect(formatOffset(-330)).toBe('UTC-05:30');
    expect(formatOffset(0)).toBe('UTC+00:00');
    expect(localDate(Date.UTC(2026, 2, 2, 23, 30), 'Asia/Tokyo')).toBe('2026-03-03');
    expect(localDate(Date.UTC(2026, 2, 2, 23, 30), 'America/Los_Angeles')).toBe('2026-03-02');
  });
});
