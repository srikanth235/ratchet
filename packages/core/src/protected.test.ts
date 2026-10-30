import { describe, expect, it } from 'vitest';
import { findProtectedViolations, globToRegExp, matchesGlob, normalizeRepoPath } from './protected.js';

describe('normalizeRepoPath', () => {
  it('normalises', () => {
    expect(normalizeRepoPath('./bench/a.js')).toBe('bench/a.js');
    expect(normalizeRepoPath('/bench/a.js')).toBe('bench/a.js');
    expect(normalizeRepoPath('bench\\a.js')).toBe('bench/a.js');
    expect(normalizeRepoPath('src/../bench/a.js')).toBe('bench/a.js');
    expect(normalizeRepoPath('bench//a.js')).toBe('bench/a.js');
  });
});

describe('matchesGlob', () => {
  it('literal paths match exactly (root only)', () => {
    expect(matchesGlob('ratchet.yaml', 'ratchet.yaml')).toBe(true);
    expect(matchesGlob('sub/ratchet.yaml', 'ratchet.yaml')).toBe(false);
    expect(matchesGlob('ratchet.yaml.bak', 'ratchet.yaml')).toBe(false);
    expect(matchesGlob('xratchet.yaml', 'ratchet.yaml')).toBe(false);
  });

  it('dir/** matches everything underneath at any depth', () => {
    expect(matchesGlob('bench/k6/search.js', 'bench/**')).toBe(true);
    expect(matchesGlob('bench/a', 'bench/**')).toBe(true);
    expect(matchesGlob('benchmark/a', 'bench/**')).toBe(false);
    expect(matchesGlob('src/bench/a', 'bench/**')).toBe(false);
    expect(matchesGlob('tests/contract/deep/er/x.py', 'tests/contract/**')).toBe(true);
    expect(matchesGlob('tests/unit/x.py', 'tests/contract/**')).toBe(false);
  });

  it('single * does not cross directories', () => {
    expect(matchesGlob('src/a.ts', 'src/*.ts')).toBe(true);
    expect(matchesGlob('src/deep/a.ts', 'src/*.ts')).toBe(false);
    expect(matchesGlob('src/a.js', 'src/*.ts')).toBe(false);
    expect(matchesGlob('src/.hidden.ts', 'src/*.ts')).toBe(true);
  });

  it('? matches exactly one non-slash char', () => {
    expect(matchesGlob('a1.txt', 'a?.txt')).toBe(true);
    expect(matchesGlob('a.txt', 'a?.txt')).toBe(false);
    expect(matchesGlob('a/.txt', 'a?.txt')).toBe(false);
  });

  it('**/ matches zero or more directories', () => {
    expect(matchesGlob('ratchet.yaml', '**/ratchet.yaml')).toBe(true);
    expect(matchesGlob('a/b/ratchet.yaml', '**/ratchet.yaml')).toBe(true);
    expect(matchesGlob('a/ratchet.yml', '**/ratchet.yaml')).toBe(false);
    expect(matchesGlob('src/x/y/test_a.py', 'src/**/test_*.py')).toBe(true);
    expect(matchesGlob('src/test_a.py', 'src/**/test_*.py')).toBe(true);
  });

  it('character classes and braces', () => {
    expect(matchesGlob('a1', 'a[0-9]')).toBe(true);
    expect(matchesGlob('ab', 'a[0-9]')).toBe(false);
    expect(matchesGlob('ab', 'a[!0-9]')).toBe(true);
    expect(matchesGlob('src/a.ts', 'src/*.{ts,js}')).toBe(true);
    expect(matchesGlob('src/a.js', 'src/*.{ts,js}')).toBe(true);
    expect(matchesGlob('src/a.py', 'src/*.{ts,js}')).toBe(false);
  });

  it('treats regex metacharacters literally', () => {
    expect(matchesGlob('a+b.txt', 'a+b.txt')).toBe(true);
    expect(matchesGlob('aab.txt', 'a+b.txt')).toBe(false);
    expect(matchesGlob('a(b).txt', 'a(b).txt')).toBe(true);
    expect(matchesGlob('a$.txt', 'a$.txt')).toBe(true);
    expect(matchesGlob('axtxt', 'a.txt')).toBe(false);
    expect(matchesGlob('x[', 'x[')).toBe(true);
  });

  it('a pattern matching a parent directory protects its children', () => {
    expect(matchesGlob('bench/k6/s.js', 'bench')).toBe(true);
    expect(matchesGlob('bench/k6/s.js', 'bench/')).toBe(true);
    expect(matchesGlob('bench/k6/s.js', './bench/k6')).toBe(true);
    expect(matchesGlob('benchx/s.js', 'bench')).toBe(false);
  });

  it('normalises the candidate path before matching (no ../ escape)', () => {
    expect(matchesGlob('./src/../bench/a.js', 'bench/**')).toBe(true);
    expect(matchesGlob('/bench/a.js', 'bench/**')).toBe(true);
    expect(matchesGlob('bench\\a.js', 'bench/**')).toBe(true);
  });

  it('is case-sensitive and ignores empty inputs', () => {
    expect(matchesGlob('Bench/a', 'bench/**')).toBe(false);
    expect(matchesGlob('', 'bench/**')).toBe(false);
    expect(matchesGlob('a', '')).toBe(false);
  });

  it('globToRegExp is anchored', () => {
    expect(globToRegExp('a*').test('xa')).toBe(false);
    expect(globToRegExp('a*').test('ab')).toBe(true);
  });
});

describe('findProtectedViolations', () => {
  const globs = ['bench/**', 'ratchet.yaml', 'tests/contract/**'];

  it('returns nothing when no path is protected', () => {
    expect(findProtectedViolations(['src/app.ts', 'README.md'], globs)).toEqual([]);
    expect(findProtectedViolations([], globs)).toEqual([]);
    expect(findProtectedViolations(['bench/x'], [])).toEqual([]);
  });

  it('reports each violating path with the first matching pattern', () => {
    expect(
      findProtectedViolations(['src/app.ts', 'bench/k6/search.js', 'ratchet.yaml', 'tests/contract/a.py'], globs),
    ).toEqual([
      { path: 'bench/k6/search.js', pattern: 'bench/**' },
      { path: 'ratchet.yaml', pattern: 'ratchet.yaml' },
      { path: 'tests/contract/a.py', pattern: 'tests/contract/**' },
    ]);
  });

  it('deduplicates paths after normalisation', () => {
    expect(findProtectedViolations(['ratchet.yaml', './ratchet.yaml'], globs)).toHaveLength(1);
  });
});
