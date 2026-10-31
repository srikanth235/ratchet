/**
 * A small, tolerant reader for the HCL subset a drafted environment is written in. It exists so the validator
 * (`validate.ts`) can look at what a setup agent wrote WITHOUT running anything: blocks (`resource "t" "n" { ... }`),
 * attributes, nested objects / lists, strings (with `${}` interpolation flagged), heredocs and comments.
 *
 * It is deliberately not a full HCL parser. Whatever it cannot read becomes an `expr` (raw text) and the validator treats an
 * expression where it needs a literal as "not pinned". The authoritative check is still `tofu validate` / `tofu plan`.
 */

export type HclValue =
  | { kind: 'string'; value: string; dynamic: boolean }
  | { kind: 'number'; value: number }
  | { kind: 'bool'; value: boolean }
  | { kind: 'null' }
  | { kind: 'object'; entries: Record<string, HclValue> }
  | { kind: 'list'; items: HclValue[] }
  | { kind: 'expr'; text: string };

export interface HclBlock {
  /** `resource`, `module`, `data`, `variable`, `output`, `provider`, `terraform`, `locals`, `lifecycle`, `provisioner`, ... */
  type: string;
  labels: string[];
  attrs: Record<string, HclValue>;
  blocks: HclBlock[];
  /** The block's source text (header and body), for pattern checks the structure does not cover. */
  raw: string;
  /** 1-based line of the block header. */
  line: number;
}

export interface HclFile {
  /** Top-level blocks. Top-level attributes do not exist in a module. */
  blocks: HclBlock[];
  /** Why the file could not be read completely (unbalanced braces, unterminated string). Empty when fine. */
  errors: string[];
}

const isSpace = (c: string | undefined): boolean => c === ' ' || c === '\t' || c === '\r';
const isIdentStart = (c: string | undefined): boolean => !!c && /[A-Za-z_]/.test(c);
const isIdent = (c: string | undefined): boolean => !!c && /[A-Za-z0-9_-]/.test(c);

/** Replace comments with spaces (keeping newlines and string contents), so offsets and line numbers stay valid. */
export function stripComments(src: string): string {
  let out = '';
  let i = 0;
  const n = src.length;
  while (i < n) {
    const c = src[i]!;
    const two = src.slice(i, i + 2);
    if (c === '"') {
      const end = skipString(src, i);
      out += src.slice(i, end);
      i = end;
    } else if (two === '<<' && heredocStart(src, i)) {
      const end = skipHeredoc(src, i);
      out += src.slice(i, end);
      i = end;
    } else if (c === '#' || two === '//') {
      while (i < n && src[i] !== '\n') {
        out += ' ';
        i++;
      }
    } else if (two === '/*') {
      const close = src.indexOf('*/', i + 2);
      const end = close === -1 ? n : close + 2;
      for (let k = i; k < end; k++) out += src[k] === '\n' ? '\n' : ' ';
      i = end;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/** Index just past the closing quote of the string starting at `i` (handles `\"` and nested `${ "..." }`). */
function skipString(src: string, i: number): number {
  let k = i + 1;
  while (k < src.length) {
    const c = src[k]!;
    if (c === '\\') k += 2;
    else if (c === '"') return k + 1;
    else if (c === '$' && src[k + 1] === '{') k = skipInterpolation(src, k + 2);
    else if (c === '\n') return k; // unterminated: stop at the line end
    else k++;
  }
  return k;
}

function skipInterpolation(src: string, from: number): number {
  let depth = 1;
  let k = from;
  while (k < src.length && depth > 0) {
    const c = src[k]!;
    if (c === '"') k = skipString(src, k);
    else {
      if (c === '{') depth++;
      else if (c === '}') depth--;
      k++;
    }
  }
  return k;
}

const HEREDOC = /^<<-?([A-Za-z_][A-Za-z0-9_]*)[ \t]*\r?\n/;
function heredocStart(src: string, i: number): RegExpExecArray | null {
  return HEREDOC.exec(src.slice(i, i + 200));
}

function skipHeredoc(src: string, i: number): number {
  const m = heredocStart(src, i);
  if (!m) return i + 2;
  const tag = m[1]!;
  let k = i + m[0].length;
  while (k < src.length) {
    const eol = src.indexOf('\n', k);
    const line = src.slice(k, eol === -1 ? src.length : eol);
    if (line.trim() === tag) return eol === -1 ? src.length : k + line.length;
    if (eol === -1) return src.length;
    k = eol + 1;
  }
  return src.length;
}

/** Index of the brace matching the `{` / `[` at `open`, or -1. Strings, heredocs and (already stripped) comments are skipped. */
function matching(src: string, open: number): number {
  const stack: string[] = [];
  let k = open;
  while (k < src.length) {
    const c = src[k]!;
    if (c === '"') {
      k = skipString(src, k);
      continue;
    }
    if (c === '<' && src[k + 1] === '<' && heredocStart(src, k)) {
      k = skipHeredoc(src, k);
      continue;
    }
    if (c === '{' || c === '[' || c === '(') stack.push(c);
    else if (c === '}' || c === ']' || c === ')') {
      const want = c === '}' ? '{' : c === ']' ? '[' : '(';
      if (stack.pop() !== want) return -1;
      if (stack.length === 0) return k;
    }
    k++;
  }
  return -1;
}

const lineOf = (src: string, offset: number): number => {
  let line = 1;
  for (let i = 0; i < offset && i < src.length; i++) if (src[i] === '\n') line++;
  return line;
};

/** Parse a whole .tf file into its top-level blocks. */
export function parseHcl(source: string): HclFile {
  const src = stripComments(source);
  const errors: string[] = [];
  const blocks = parseBody(src, 0, src.length, source, errors).blocks;
  return { blocks, errors };
}

interface Body {
  attrs: Record<string, HclValue>;
  blocks: HclBlock[];
}

function parseBody(src: string, from: number, to: number, original: string, errors: string[]): Body {
  const attrs: Record<string, HclValue> = {};
  const blocks: HclBlock[] = [];
  let i = from;
  const skipWs = () => {
    while (i < to && (isSpace(src[i]) || src[i] === '\n' || src[i] === ',')) i++;
  };
  while (i < to) {
    skipWs();
    if (i >= to) break;
    if (!isIdentStart(src[i])) {
      errors.push(`unexpected ${JSON.stringify(src[i])} on line ${lineOf(original, i)}`);
      // resynchronise on the next line
      const nl = src.indexOf('\n', i);
      i = nl === -1 || nl >= to ? to : nl + 1;
      continue;
    }
    const start = i;
    while (i < to && isIdent(src[i])) i++;
    const word = src.slice(start, i);
    while (i < to && isSpace(src[i])) i++;
    if (src[i] === '=' && src[i + 1] !== '=') {
      i++;
      const end = valueEnd(src, i, to);
      if (end < 0) {
        errors.push(`unbalanced value for "${word}" on line ${lineOf(original, start)}`);
        break;
      }
      attrs[word] = parseValue(src, i, end, original, errors);
      i = end;
      continue;
    }
    if (src[i] === ':' && src[i + 1] !== ':') {
      // object-constructor style `key: value`
      i++;
      const end = valueEnd(src, i, to);
      if (end < 0) break;
      attrs[word] = parseValue(src, i, end, original, errors);
      i = end;
      continue;
    }
    // a block: labels (quoted or bare), then `{`
    const labels: string[] = [];
    for (;;) {
      while (i < to && isSpace(src[i])) i++;
      if (src[i] === '"') {
        const end = skipString(src, i);
        labels.push(src.slice(i + 1, end - 1));
        i = end;
      } else if (isIdentStart(src[i])) {
        const s = i;
        while (i < to && isIdent(src[i])) i++;
        labels.push(src.slice(s, i));
      } else break;
    }
    if (src[i] !== '{') {
      errors.push(`expected "{" after "${word}" on line ${lineOf(original, start)}`);
      const nl = src.indexOf('\n', i);
      i = nl === -1 || nl >= to ? to : nl + 1;
      continue;
    }
    const close = matching(src, i);
    if (close < 0 || close >= to) {
      errors.push(`block "${word}" starting on line ${lineOf(original, start)} is never closed`);
      break;
    }
    const inner = parseBody(src, i + 1, close, original, errors);
    blocks.push({ type: word, labels, attrs: inner.attrs, blocks: inner.blocks, raw: original.slice(start, close + 1), line: lineOf(original, start) });
    i = close + 1;
  }
  return { attrs, blocks };
}

/** End offset of the expression starting at `from`: the end of the line, or of the bracketed group that spans lines. */
function valueEnd(src: string, from: number, to: number): number {
  let k = from;
  while (k < to) {
    const c = src[k]!;
    if (c === '"') {
      k = skipString(src, k);
      continue;
    }
    if (c === '<' && src[k + 1] === '<' && heredocStart(src, k)) return Math.min(skipHeredoc(src, k), to);
    if (c === '{' || c === '[' || c === '(') {
      const close = matching(src, k);
      if (close < 0) return -1;
      k = close + 1;
      continue;
    }
    if (c === ',') return k; // inside an object constructor: `{ a = 1, b = 2 }`
    if (c === '\n') {
      // a trailing operator / comma continues the expression on the next line
      let b = k - 1;
      while (b >= from && isSpace(src[b])) b--;
      if (b >= from && /[+\-*/?:&|,=<>!]/.test(src[b]!) && src[b] !== ',') {
        k++;
        continue;
      }
      return k;
    }
    k++;
  }
  return to;
}

function parseValue(src: string, from: number, to: number, original: string, errors: string[]): HclValue {
  let a = from;
  let b = to;
  while (a < b && (isSpace(src[a]) || src[a] === '\n')) a++;
  while (b > a && (isSpace(src[b - 1]) || src[b - 1] === '\n' || src[b - 1] === ',')) b--;
  const text = src.slice(a, b);
  if (text === '') return { kind: 'expr', text: '' };
  if (text.startsWith('"')) {
    const end = skipString(src, a);
    if (end === b) {
      const inner = src.slice(a + 1, b - 1);
      const dynamic = /\$\{|%\{/.test(inner);
      return { kind: 'string', value: dynamic ? inner : unescape(inner), dynamic };
    }
    return { kind: 'expr', text };
  }
  if (text.startsWith('<<') && heredocStart(src, a)) {
    const m = heredocStart(src, a)!;
    const body = text.slice(m[0].length);
    const lines = body.split('\n');
    lines.pop(); // closing tag
    const value = lines.join('\n');
    return { kind: 'string', value, dynamic: /\$\{|%\{/.test(value) };
  }
  if (text.startsWith('{') && matching(src, a) === b - 1) {
    return { kind: 'object', entries: parseBody(src, a + 1, b - 1, original, errors).attrs };
  }
  if (text.startsWith('[') && matching(src, a) === b - 1) {
    return { kind: 'list', items: splitList(src, a + 1, b - 1).map(([s, e]) => parseValue(src, s, e, original, errors)) };
  }
  if (text === 'true' || text === 'false') return { kind: 'bool', value: text === 'true' };
  if (text === 'null') return { kind: 'null' };
  if (/^-?\d+(\.\d+)?$/.test(text)) return { kind: 'number', value: Number(text) };
  return { kind: 'expr', text };
}

function splitList(src: string, from: number, to: number): Array<[number, number]> {
  const out: Array<[number, number]> = [];
  let start = from;
  let k = from;
  while (k < to) {
    const c = src[k]!;
    if (c === '"') {
      k = skipString(src, k);
      continue;
    }
    if (c === '{' || c === '[' || c === '(') {
      const close = matching(src, k);
      k = close < 0 ? to : close + 1;
      continue;
    }
    if (c === ',') {
      out.push([start, k]);
      start = k + 1;
    }
    k++;
  }
  if (src.slice(start, to).trim() !== '') out.push([start, to]);
  return out;
}

function unescape(s: string): string {
  return s.replace(/\\(["\\nrt])/g, (_m, c: string) => (c === 'n' ? '\n' : c === 'r' ? '\r' : c === 't' ? '\t' : c));
}

// ---- helpers for the validator ----------------------------------------------------------------------------------------

/** The literal string of a value, or undefined when it is not a plain (non-interpolated) string. */
export function literalString(v: HclValue | undefined): string | undefined {
  return v?.kind === 'string' && !v.dynamic ? v.value : undefined;
}

export function literalNumber(v: HclValue | undefined): number | undefined {
  return v?.kind === 'number' ? v.value : undefined;
}

/** Every block (this one included) in a tree, depth first. */
export function* walkBlocks(blocks: readonly HclBlock[]): Generator<HclBlock> {
  for (const b of blocks) {
    yield b;
    yield* walkBlocks(b.blocks);
  }
}
