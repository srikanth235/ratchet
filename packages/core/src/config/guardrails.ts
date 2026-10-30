import type { Comparator, Guardrail, ThresholdGuardrail } from './schema.js';

/** Result of evaluating one guardrail; this is the shape the ratchet rule consumes. */
export interface GuardrailResult {
  /** Human-readable name, e.g. `pytest -q` or `error_rate < 0.001`. */
  name: string;
  passed: boolean;
  detail?: string;
}

export function guardrailName(g: Guardrail): string {
  return g.kind === 'command' ? g.run : `${g.metric} ${g.raw}`;
}

function compare(a: number, op: Comparator, b: number): boolean {
  switch (op) {
    case '<':
      return a < b;
    case '<=':
      return a <= b;
    case '>':
      return a > b;
    case '>=':
      return a >= b;
    case '==':
      return a === b;
  }
}

/**
 * Evaluate a threshold guardrail. For relative bounds ("< 1.2x") the bound is
 * `value * baseline`, so `baseline` is required. A non-finite observation fails.
 */
export function evaluateThreshold(
  g: ThresholdGuardrail,
  observed: number,
  baseline?: number,
): GuardrailResult {
  const name = guardrailName(g);
  if (!Number.isFinite(observed)) {
    return { name, passed: false, detail: `${g.metric} was not a finite number (${observed})` };
  }
  let bound = g.value;
  if (g.relative) {
    if (baseline === undefined || !Number.isFinite(baseline)) {
      return { name, passed: false, detail: `${g.metric} needs a baseline value to evaluate "${g.raw}"` };
    }
    bound = g.value * baseline;
  }
  const passed = compare(observed, g.op, bound);
  return {
    name,
    passed,
    detail: `${g.metric} = ${observed}, required ${g.op} ${bound}${g.relative ? ` (${g.value}x baseline ${baseline})` : ''}`,
  };
}

/** Exit-code based evaluation for command guardrails. */
export function evaluateCommandResult(g: { run: string }, exitCode: number): GuardrailResult {
  return {
    name: g.run,
    passed: exitCode === 0,
    detail: exitCode === 0 ? undefined : `exited with status ${exitCode}`,
  };
}
