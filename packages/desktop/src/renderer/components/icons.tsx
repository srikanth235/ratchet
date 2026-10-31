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
  overview: <><path d="M2.5 11.5a5.5 5.5 0 1 1 11 0" /><path d="M8 11.5l2.8-3.6" /></>,
  optimizations: <path d="M2 13.5 L5.5 6 V13.5 L9 6 V13.5 L12.5 6 V13.5" />,
  benchmarks: <path d="M3 13.5V9M6.3 13.5V5M9.7 13.5V7.5M13 13.5V3" />,
  settings: <path d="M6 10l4-4M5 7.5 3.5 9a2.5 2.5 0 0 0 3.5 3.5L8.5 11M11 8.5 12.5 7A2.5 2.5 0 0 0 9 3.5L7.5 5" />,
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
    <svg width="22" height="22" viewBox="0 0 18 18" fill="none" stroke="#D9B26F" strokeWidth="1.7" strokeLinejoin="round" aria-hidden="true">
      <path d="M2 13.5 L6 5 V13.5 L10 5 V13.5 L14 5 V13.5" />
    </svg>
  );
}
