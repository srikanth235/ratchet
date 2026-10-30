import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { formatConfigError, getTemplate, listTemplates, parseConfig, renderTemplate, resolveTemplateId, type TemplateInfo } from '@ratchet/core';
import type { Io } from '../io.js';

export interface InitArgs {
  /** A template id (see `--list-templates`). */
  template?: string;
  /** Legacy alias of `template` (`perf` and `forecasting` map to the templates that replaced them). */
  pack?: string;
  metric?: string;
  direction?: string;
  valueUsd?: number;
  /** Directory to write into (default: the cwd). */
  dir?: string;
  force?: boolean;
  dryRun?: boolean;
  listTemplates?: boolean;
  /** Legacy alias of `listTemplates`. */
  listPacks?: boolean;
}

function describe(t: TemplateInfo): string {
  if (t.kind === 'demo') {
    return [`${t.id}  [demo]  ${t.title}`, `    ${t.description}`, `    try it: ${t.example} (runs in simulation, nothing to set up)`].join('\n');
  }
  return [
    `${t.id}  ${t.title}`,
    `    ${t.description}`,
    `    measures ${t.measures}; ${t.holdout ? `holdout: ${t.holdout}` : 'no holdout'}; machine ${t.machine}`,
    `    default: ${t.objective.direction} ${t.objective.metric}; writes ${t.files.join(', ')}`,
  ].join('\n');
}

/** `ratchet init --list-templates` */
function listAll(io: Io): number {
  const groups: Array<[string, TemplateInfo[]]> = [
    ['Templates (ratchet init --template <id>)', listTemplates({ kind: 'template' })],
    ['Try a demo', listTemplates({ kind: 'demo' })],
  ];
  io.out(groups.map(([title, items]) => `${title}\n\n${items.map(describe).join('\n\n')}`).join('\n\n') + '\n');
  return 0;
}

/**
 * `ratchet init --template <id>` (alias `--pack`): write ratchet.yaml, RATCHET.md and the evaluator's files into `dir`.
 * The generated ratchet.yaml is checked with the same parser as `ratchet validate` before anything is written.
 * Returns the process exit code: 0 written (or dry run), 1 refused or invalid.
 */
export function runInit(args: InitArgs, io: Io): number {
  if (args.listTemplates || args.listPacks) return listAll(io);
  const requested = args.template ?? args.pack;
  const ids = listTemplates({ kind: 'template' }).map((t) => t.id);
  if (!requested) {
    io.err(`init: --template is required (one of ${ids.join(', ')}; see --list-templates)\n`);
    return 1;
  }
  const info = getTemplate(requested);
  if (!info) {
    io.err(`init: unknown template ${JSON.stringify(requested)} (available: ${ids.join(', ')})\n`);
    return 1;
  }
  if (info.kind === 'demo') {
    io.err(`init: ${info.id} is a demo, not a template: it runs in simulation from ${info.example}; there is nothing to write into a repository\n`);
    return 1;
  }

  const direction = args.direction ?? info.objective.direction;
  if (direction !== 'minimize' && direction !== 'maximize') {
    io.err(`init: --direction must be "minimize" or "maximize", got ${JSON.stringify(direction)}\n`);
    return 1;
  }
  if (args.valueUsd !== undefined && (!Number.isFinite(args.valueUsd) || args.valueUsd <= 0)) {
    io.err('init: --value-usd must be a positive number\n');
    return 1;
  }

  const rendered = renderTemplate(info.id, { ...(args.metric?.trim() ? { metric: args.metric.trim() } : {}), direction, ...(args.valueUsd !== undefined ? { valueUsd: args.valueUsd } : {}) });
  const { files, options } = rendered;
  const generated = files.find((f) => f.path === 'ratchet.yaml')!;
  const parsed = parseConfig(generated.content);
  if (!parsed.ok) {
    io.err(`init: the generated ratchet.yaml is invalid (check the options):\n`);
    for (const e of parsed.errors) io.err(`  - ${formatConfigError(e)}\n`);
    return 1;
  }

  const root = resolve(args.dir ?? '.');
  const existing = files.filter((f) => existsSync(join(root, f.path)));
  if (existing.length > 0 && !args.force) {
    io.err(`init: refusing to overwrite ${existing.map((f) => f.path).join(', ')} (use --force)\n`);
    return 1;
  }

  const verb = args.dryRun ? 'would write' : 'wrote';
  const alias = args.pack && !args.template && resolveTemplateId(args.pack) !== args.pack ? ` (--pack ${args.pack} is now the ${info.id} template)` : '';
  const lines = files.map((f) => `  ${verb} ${f.path}${existing.includes(f) ? ' (overwritten)' : ''}  ${f.content.split('\n').length - 1} lines`);
  if (args.dryRun) {
    io.out(`ratchet init --template ${info.id}${alias} (dry run, nothing written to ${root})\n${lines.join('\n')}\n\n--- ratchet.yaml ---\n${generated.content}`);
    return 0;
  }
  for (const f of files) {
    const target = join(root, f.path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, f.content);
  }
  io.out(
    `ratchet init --template ${info.id}${alias}: ${options.direction} ${options.metric}, $${options.valueUsd} per ${info.objective.unit} per year (estimated)\n${lines.join('\n')}\n\n` +
      `Next: replace the TODOs, then \`ratchet validate\` and \`ratchet baseline\`.\n`,
  );
  return 0;
}
