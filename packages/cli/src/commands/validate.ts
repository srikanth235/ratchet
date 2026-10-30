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
    `  bench       ${c.bench.run}`,
    `              ref ${c.bench.ref}, ${c.bench.repeats} repeats on ${c.bench.hardware}` +
      (c.bench.confirm_trace ? `, confirm trace ${c.bench.confirm_trace}` : ', no confirm trace'),
    `  guardrails  ${c.guardrails.length === 0 ? 'none' : c.guardrails.map(guardrailName).join('; ')}`,
    `  protected   ${c.protected.length === 0 ? 'none' : c.protected.join(', ')}`,
    `  agent       ${c.agent.instructions}; instance types ${c.agent.allowed_instance_types.join(', ')}`,
  ];
  io.out(lines.join('\n') + '\n');
  return 0;
}
