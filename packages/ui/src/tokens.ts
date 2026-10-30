/** Midnight Brass design tokens (docs/V0_ARCHITECTURE.md §9). Mirrored in tokens.css. */
export const tokens = {
  color: {
    window: '#0B1019',
    sidebar: '#0E1420',
    card: '#111827',
    hairline: 'rgba(148,163,190,.13)',
    text: '#ECEFF4',
    textSecondary: '#A7B1C2',
    textTertiary: '#8592A6',
    brass: '#D9B26F',
    brassHighlight: '#EBCB8E',
    mint: '#7BD8B0',
    rose: '#F08A93',
    claudeCode: '#B8A6FF',
    codex: '#7CC4F2',
  },
  font: {
    serif: "'Instrument Serif', Georgia, serif",
    sans: "'Instrument Sans', system-ui, sans-serif",
    mono: "'JetBrains Mono', ui-monospace, monospace",
  },
  radius: { card: '14px', control: '10px', controlSm: '8px', pill: '999px' },
  space: { 1: '4px', 2: '8px', 3: '12px', 4: '16px', 5: '20px', 6: '24px', 8: '32px' },
} as const;

export type Tokens = typeof tokens;

const kebab = (s: string) => s.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);

/** Flatten tokens to `--rt-*` custom properties (same names as tokens.css). */
export function tokensToCssVars(t: Tokens = tokens): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [group, values] of Object.entries(t)) {
    const prefix = group === 'space' ? 'space' : group;
    for (const [k, v] of Object.entries(values)) out[`--rt-${prefix}-${kebab(k)}`] = v;
  }
  return out;
}
