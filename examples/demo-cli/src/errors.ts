import { t } from './messages.js';
import type { Vars } from './icu.js';

/** A user-facing failure with a catalog code (`errors.E0201`), printed as `taskr: error E0201: ...`. */
export class TaskrError extends Error {
  constructor(
    readonly code: string,
    vars?: Vars,
    readonly exitCode = 1,
  ) {
    super(t(`errors.${code}`, vars));
  }
}
