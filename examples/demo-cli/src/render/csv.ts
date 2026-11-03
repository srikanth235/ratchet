/** RFC 4180 style CSV: fields with a comma, quote or newline are quoted, quotes doubled. */
export function csvField(value: string | number | null): string {
  const s = value === null ? '' : String(value);
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export function renderCsv(header: string[], rows: (string | number | null)[][]): string {
  return [header, ...rows].map((r) => r.map(csvField).join(',')).join('\n') + '\n';
}
