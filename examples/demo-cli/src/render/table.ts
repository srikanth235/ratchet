export interface Column {
  title: string;
  align?: 'left' | 'right';
  /** The flexible column absorbs the width left over and is truncated with an ellipsis; at most one. */
  flex?: boolean;
}

function fit(text: string, width: number, align: 'left' | 'right'): string {
  const cut = text.length > width ? `${text.slice(0, Math.max(0, width - 1))}…` : text;
  return align === 'right' ? cut.padStart(width) : cut.padEnd(width);
}

/** Lay out rows as a space-separated table no wider than `maxWidth` columns. */
export function renderTable(columns: Column[], rows: string[][], maxWidth: number): string {
  const widths = columns.map((c, i) => Math.max(c.title.length, ...rows.map((r) => (r[i] ?? '').length)));
  const flex = columns.findIndex((c) => c.flex);
  if (flex >= 0) {
    const others = widths.reduce((sum, w, i) => (i === flex ? sum : sum + w), 0) + (columns.length - 1) * 2;
    widths[flex] = Math.max(8, Math.min(widths[flex]!, maxWidth - others));
  }
  const line = (cells: string[]) =>
    cells
      .map((cell, i) => fit(cell, widths[i]!, columns[i]!.align ?? 'left'))
      .join('  ')
      .trimEnd();
  return [line(columns.map((c) => c.title)), ...rows.map(line)].join('\n') + '\n';
}

/** A horizontal bar of `width` cells filled in proportion to `value / max`. */
export function bar(value: number, max: number, width: number, unicode: boolean): string {
  const filled = max > 0 ? Math.round((value / max) * width) : 0;
  return unicode ? '█'.repeat(filled) + '·'.repeat(width - filled) : '#'.repeat(filled) + '.'.repeat(width - filled);
}
