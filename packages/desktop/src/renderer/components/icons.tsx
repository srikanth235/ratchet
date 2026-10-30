import type { ReactNode } from 'react';

/** 24-grid line icons for buttons and inline use. */
const P = {
  plus: <path d="M12 5v14M5 12h14" />,
  send: <path d="M4 12l16-8-6 16-3-7-7-1z" />,
  pause: <path d="M9 6v12M15 6v12" />,
  play: <path d="M8 5l11 7-11 7z" />,
  stop: <rect x="6" y="6" width="12" height="12" rx="2" />,
  check: <path d="M5 12.5l4.5 4.5L19 7.5" />,
  warn: <><path d="M12 4l9 16H3z" /><path d="M12 10v4M12 17v.5" /></>,
  clock: <><circle cx="12" cy="12" r="8" /><path d="M12 8v4.5l3 1.5" /></>,
  external: <path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 01-1 1H5a1 1 0 01-1-1V7a1 1 0 011-1h5" />,
  chevron: <path d="M9 6l6 6-6 6" />,
  message: <path d="M5 5h14v10H10l-4 4v-4H5z" />,
  terminal: <><rect x="3" y="4" width="18" height="16" rx="3" /><path d="M7.5 10l3 2-3 2M13 15h4" /></>,
  branch: <><circle cx="7" cy="6" r="2" /><circle cx="7" cy="18" r="2" /><circle cx="17" cy="9" r="2" /><path d="M7 8v8M17 11c0 4-6 3-10 5" /></>,
  arrow: <path d="M5 12h14M13 6l6 6-6 6" />,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof P;

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {P[name]}
    </svg>
  );
}

/** The 16-grid sidebar glyphs from the design (stroke 1.5). Keys are the route ids. */
const NAV: Record<string, ReactNode> = {
  optimizations: <><path d="M2.5 11.5a5.5 5.5 0 1 1 11 0" /><path d="M8 11.5l2.8-3.6" /><circle cx="8" cy="11.5" r="0.9" fill="currentColor" /></>,
  benchmarks: <path d="M3 13.5V9M6.3 13.5V5M9.7 13.5V7.5M13 13.5V3" />,
  agents: <><rect x="2" y="3" width="12" height="10" rx="2" /><path d="M5 6.5l2 1.5-2 1.5M8.5 10H11" /></>,
  sandbox: <path d="M4.5 12.5h7a3 3 0 0 0 .3-6A4 4 0 0 0 4.2 7.6 2.5 2.5 0 0 0 4.5 12.5z" />,
  result: <><path d="M4 2h8v12l-2-1-2 1-2-1-2 1z" /><path d="M6 5.5h4M6 8.5h4" /></>,
  settings: <><path d="M2.5 5h11M2.5 11h11" /><circle cx="6" cy="5" r="1.8" fill="var(--rt-color-sidebar)" /><circle cx="10.5" cy="11" r="1.8" fill="var(--rt-color-sidebar)" /></>,
};

export function NavIcon({ id }: { id: string }) {
  return (
    <svg className="nav__icon" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor"
      strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {NAV[id]}
    </svg>
  );
}

export function RatchetMark() {
  return (
    <svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="#D9B26F" strokeWidth="1.8" strokeLinejoin="round" aria-hidden="true">
      <path d="M2 13.5 L6 5 V13.5 L10 5 V13.5 L14 5 V13.5" />
    </svg>
  );
}
