import { formatConfigError, guardrailName, loadConfig } from '@ratchet/core';
import type { Io } from '../io.js';

export const DEFAULT_CONFIG_PATH = 'ratchet.yaml';

/** `ratchet validate [path]`. Returns the process exit code: 0 valid, 1 invalid. */
export function runValidate(path: string | undefined, io: Io): number {
  const file = path ?? DEFAULT_CONFIG_PATH;
  const res = loadConfig(file);
  if (!res.ok) {
    io.err(`${file}: invalid (${res.errors.length} error${res.errors.length === 1 ? '' : 's'})\n`);
    for (const e of res.errors) io.err(`  - ${formatConfigError(e)}\n`);
    return 1;
  }
  const c = res.config;
  const lines = [
    `${file}: OK`,
    `  objective   ${c.objective.direction} ${c.objective.metric} (min gain ${(c.objective.min_gain * 100).toFixed(2).replace(/\.?0+$/, '')}%)`,
    ...(c.pack ? [`  pack        ${c.pack}`] : []),
    ...(c.objective.value
      ? [`  value       $${c.objective.value.usd_per_unit} per ${c.objective.value.unit ?? 'unit'} per ${c.objective.value.period} (${c.objective.value.basis})`]
      : []),
    `  evaluator   ${c.evaluator.kind}: ${c.evaluator.run}`,
    `              ref ${c.evaluator.ref}, ${c.evaluator.repeats} repeats on ${c.evaluator.hardware}` +
      (c.evaluator.holdout ? `, holdout ${c.evaluator.holdout}` : ', no holdout'),
    `  guardrails  ${c.guardrails.length === 0 ? 'none' : c.guardrails.map(guardrailName).join('; ')}`,
    `  protected   ${c.protected.length === 0 ? 'none' : c.protected.join(', ')}`,
    `  agent       ${c.agent.instructions}; instance types ${c.agent.allowed_instance_types.join(', ')}`,
  ];
  io.out(lines.join('\n') + '\n');
  return 0;
}
