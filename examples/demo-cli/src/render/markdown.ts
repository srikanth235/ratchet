/**
 * Markdown output. Cell text is escaped with a table of rules compiled once, when the module loads: each
 * special character gets a pattern for every position it is special in.
 */
const SPECIAL = '\\`*_{}[]()#+-.!|<>~^&$%@=:;"\'/?,'.split('');
const POSITIONS: { name: string; pattern: (c: string) => string }[] = [
  { name: 'line start', pattern: (c) => `^(\\s*)\\${c}` },
  { name: 'after space', pattern: (c) => `(\\s)\\${c}(?=\\w)` },
  { name: 'doubled', pattern: (c) => `\\${c}\\${c}` },
];

interface Rule {
  regex: RegExp;
  replace: string;
}

const RULES: Rule[] = [];
for (const c of SPECIAL) {
  for (const p of POSITIONS) {
    RULES.push({ regex: new RegExp(p.pattern(c), 'g'), replace: p.name === 'doubled' ? `\\${c}\\${c}` : `$1\\${c}` });
  }
}
/** Only these actually need a backslash inside a table cell; the broader rule table above is kept for prose. */
const CELL_ESCAPES = /[|\\]/g;

export function escapeCell(text: string): string {
  return text.replace(CELL_ESCAPES, (c) => `\\${c}`);
}

export function escapeProse(text: string): string {
  let out = text;
  for (const rule of RULES) out = out.replace(rule.regex, rule.replace);
  return out;
}

export function renderMarkdownTable(header: string[], rows: string[][]): string {
  const line = (cells: string[]) => `| ${cells.map(escapeCell).join(' | ')} |`;
  return [line(header), `| ${header.map(() => '---').join(' | ')} |`, ...rows.map(line)].join('\n') + '\n';
}
