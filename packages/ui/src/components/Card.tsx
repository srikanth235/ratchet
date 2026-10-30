import type { HTMLAttributes, ReactNode } from 'react';

export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  children?: ReactNode;
}

export function Card({ className, children, ...rest }: CardProps) {
  return (
    <div className={['rt-card', className].filter(Boolean).join(' ')} {...rest}>
      {children}
    </div>
  );
}

export function Eyebrow({ className, children, ...rest }: HTMLAttributes<HTMLSpanElement>) {
  return (
    <span className={['rt-eyebrow', className].filter(Boolean).join(' ')} {...rest}>
      {children}
    </span>
  );
}
