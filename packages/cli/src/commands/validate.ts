import { effectiveProtected, formatConfigError, guardrailName, loadConfig } from '@ratchet/core';
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
  const auto = effectiveProtected(c, { configPath: file.split(/[\\/]/).pop() });
  const lines = [
    `${file}: OK`,
    `  objective   ${c.objective.direction} ${c.objective.metric} (${
      c.objective.min_gain === undefined ? 'min gain calibrated from the baseline' : `min gain ${(c.objective.min_gain * 100).toFixed(2).replace(/\.?0+$/, '')}%`
    })`,
    ...(c.pack ? [`  pack        ${c.pack}`] : []),
    ...(c.objective.value
      ? [`  value       $${c.objective.value.usd_per_unit} per ${c.objective.value.unit ?? 'unit'} per ${c.objective.value.period} (${c.objective.value.basis})`]
      : []),
    `  evaluator   ${c.evaluator.kind}: ${c.evaluator.run}${c.evaluator.template ? `  (template ${c.evaluator.template})` : ''}`,
    `              ref ${c.evaluator.ref}, ${c.evaluator.repeats === undefined ? 'repeats calibrated from the baseline' : `${c.evaluator.repeats} repeats`} on ${c.evaluator.hardware}` +
      (c.evaluator.holdout ? `, holdout ${c.evaluator.holdout}` : ', no holdout'),
    `  guardrails  ${c.guardrails.length === 0 ? 'none' : c.guardrails.map(guardrailName).join('; ')}`,
    `  protected   ${c.protected.length === 0 ? 'none listed' : c.protected.join(', ')}`,
    `              automatic: ${[...new Set(auto.entries.filter((e) => e.source !== 'user' && e.source !== 'install').map((e) => e.glob))].join(', ')}, lockfiles and install config`,
    ...(c.environment && Object.keys(c.environment).length > 0
      ? [`  environment ${Object.entries(c.environment).map(([k, v]) => `${k} ${Array.isArray(v) ? v.join(',') : String(v)}`).join('; ')}`]
      : []),
    ...(c.infra ? [`  infra       ${c.infra} (read from the protected ref only)`] : []),
    `  agent       ${c.agent.instructions}; instance types ${c.agent.allowed_instance_types.join(', ')}`,
  ];
  io.out(lines.join('\n') + '\n');
  return 0;
}
