import { BenchError } from './errors.js';

/**
 * Small shell helpers and constants shared by every generated script (bench steps, the dependency-snapshot job and the
 * container image job). Kept free of other imports from this package so that none of the script builders import each other
 * in a cycle.
 */

export const SENTINEL = '@@RATCHET@@';

/**
 * Unprivileged OS user that runs setup, guardrails and every process of the CANDIDATE (its server, its build).
 * Created and IMDS-blocked by the instance's user-data (packages/aws `benchUserData`).
 */
export const BENCH_USER = 'ratchet-bench';
/**
 * Second unprivileged user: the evaluator harness (`evaluator.run`, trusted code from bench.ref) runs as this
 * user and is the only one that can open the holdout (file mode 0600). Also IMDS-blocked by the user-data.
 */
export const HOLDOUT_USER = 'ratchet-holdout';

/** POSIX single-quote a string. */
export function sq(s: string): string {
  if (s.includes('\0')) throw new BenchError('NUL byte in a shell argument');
  return `'${s.replace(/'/g, `'\\''`)}'`;
}

const SAFE_REF = /^[A-Za-z0-9][A-Za-z0-9._\/@{}~^+-]*$/;

/** A sha / ref is interpolated into git commands; keep it to plain ref characters and never leading '-'. */
export function assertSafeRef(ref: string, what: string): string {
  if (!SAFE_REF.test(ref) || ref.includes('..')) throw new BenchError(`${what} ${JSON.stringify(ref)} is not a plain git ref or sha`);
  return ref;
}
