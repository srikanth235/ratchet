import { createRng } from './rng.js';

/**
 * The spelling dictionary behind "did you mean": the command names plus the project vocabulary (terms the
 * tasks, labels and people are written with). It is indexed by character trigrams at module load, so a
 * lookup only has to score the words that share a trigram with the input.
 */
export const COMMAND_NAMES = ['config', 'due', 'export', 'help', 'list', 'search', 'show', 'stats', 'version'] as const;

const ONSETS = ['b', 'c', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'm', 'n', 'p', 'r', 's', 't', 'v', 'w', 'br', 'cl', 'cr', 'dr', 'fl', 'gr', 'pl', 'pr', 'sc', 'sl', 'sp', 'st', 'tr', 'ch', 'sh', 'th'];
const NUCLEI = ['a', 'e', 'i', 'o', 'u', 'ai', 'ea', 'ee', 'io', 'oa', 'oo', 'ou', 'ue'];
const CODAS = ['', '', 'b', 'd', 'g', 'k', 'l', 'm', 'n', 'p', 'r', 's', 't', 'x', 'ld', 'lt', 'mp', 'nd', 'nk', 'nt', 'rd', 'rk', 'rn', 'rt', 'sk', 'st'];

function buildVocabulary(size: number): string[] {
  const rng = createRng(20240611);
  const words = new Set<string>(COMMAND_NAMES);
  const syllable = () => ONSETS[Math.floor(rng() * ONSETS.length)]! + NUCLEI[Math.floor(rng() * NUCLEI.length)]! + CODAS[Math.floor(rng() * CODAS.length)]!;
  while (words.size < size) {
    const parts = 2 + Math.floor(rng() * 2);
    let word = '';
    for (let i = 0; i < parts; i++) word += syllable();
    words.add(word);
  }
  return [...words].sort();
}

function trigrams(word: string): string[] {
  const padded = `^${word}$`;
  const grams: string[] = [];
  for (let i = 0; i + 3 <= padded.length; i++) grams.push(padded.slice(i, i + 3));
  return grams;
}

function buildIndex(words: string[]): Map<string, number[]> {
  const index = new Map<string, number[]>();
  words.forEach((word, i) => {
    for (const gram of new Set(trigrams(word))) {
      const list = index.get(gram);
      if (list) list.push(i);
      else index.set(gram, [i]);
    }
  });
  return index;
}

export const VOCABULARY: string[] = buildVocabulary(7_000);
const INDEX = buildIndex(VOCABULARY);

/** Damerau-Levenshtein distance, abandoning early once it must exceed `max`. */
export function editDistance(a: string, b: string, max = 3): number {
  if (Math.abs(a.length - b.length) > max) return max + 1;
  let prev2: number[] = [];
  let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const row = [i];
    let best = i;
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      let d = Math.min(prev[j]! + 1, row[j - 1]! + 1, prev[j - 1]! + cost);
      if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) d = Math.min(d, prev2[j - 2]! + 1);
      row.push(d);
      best = Math.min(best, d);
    }
    if (best > max) return max + 1;
    prev2 = prev;
    prev = row;
  }
  return prev[b.length]!;
}

/** Words within two edits of `input`, nearest first (ties alphabetical). `only` restricts the candidates. */
export function suggest(input: string, limit = 3, only?: readonly string[]): string[] {
  const word = input.toLowerCase();
  const seen = new Set<number>();
  for (const gram of trigrams(word)) for (const i of INDEX.get(gram) ?? []) seen.add(i);
  const scored: { word: string; d: number }[] = [];
  for (const i of seen) {
    const candidate = VOCABULARY[i]!;
    if (only && !only.includes(candidate)) continue;
    const d = editDistance(word, candidate, 2);
    if (d <= 2 && candidate !== word) scored.push({ word: candidate, d });
  }
  scored.sort((a, b) => a.d - b.d || (a.word < b.word ? -1 : 1));
  return scored.slice(0, limit).map((s) => s.word);
}
