import type { ButtonHTMLAttributes } from 'react';

export type ButtonVariant = 'primary' | 'ghost' | 'subtle' | 'danger' | 'danger-outline';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
}

/** primary = brass call to action; ghost = outlined header action; subtle = compact in-card action; danger = stop / revoke. */
export function Button({ variant = 'primary', className, type = 'button', ...rest }: ButtonProps) {
  return (
    <button
      type={type}
      className={['rt-btn', `rt-btn--${variant}`, className].filter(Boolean).join(' ')}
      {...rest}
    />
  );
}
