import type { ReactNode } from 'react';

const P = {
  optimizations: <path d="M3 17h4v-4h4V9h4V5h4" />,
  benchmarks: <path d="M4 20V10M10 20V4M16 20v-8M22 20H2" />,
  agents: <><circle cx="12" cy="8" r="3.5" /><path d="M5 20c0-4 3-6 7-6s7 2 7 6" /></>,
  sandbox: <><path d="M4 8l8-4 8 4-8 4-8-4z" /><path d="M4 12l8 4 8-4M4 16l8 4 8-4" /></>,
  result: <><circle cx="12" cy="12" r="8" /><path d="M8.5 12.5l2.5 2.5 4.5-5" /></>,
  settings: <><circle cx="12" cy="12" r="3" /><path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" /></>,
  new: <path d="M12 5v14M5 12h14" />,
  plus: <path d="M12 5v14M5 12h14" />,
  chevron: <path d="M8 10l4 4 4-4" />,
  send: <path d="M4 12l16-8-6 16-3-7-7-1z" />,
  pause: <path d="M9 6v12M15 6v12" />,
  play: <path d="M8 5l11 7-11 7z" />,
  stop: <rect x="6" y="6" width="12" height="12" rx="2" />,
  check: <path d="M5 12.5l4.5 4.5L19 7.5" />,
  external: <path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 01-1 1H5a1 1 0 01-1-1V7a1 1 0 011-1h5" />,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof P;

export function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {P[name]}
    </svg>
  );
}
