import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { formatConfigError, parseConfig } from '@ratchet/core';
import type { Io } from '../io.js';
import { findPack, PACKS, type Direction, type InitOptions, type TemplateFile } from './init-templates.js';

export interface InitArgs {
  pack?: string;
  metric?: string;
  direction?: string;
  valueUsd?: number;
  /** Directory to write into (default: the cwd). */
  dir?: string;
  force?: boolean;
  dryRun?: boolean;
  listPacks?: boolean;
}

/** `ratchet init --list-packs` */
function listPacks(io: Io): number {
  const width = Math.max(...PACKS.map((p) => p.id.length));
  io.out(
    PACKS.map(
      (p) =>
        `${p.id.padEnd(width)}  ${p.description}\n${' '.repeat(width)}  default: ${p.defaults.direction} ${p.defaults.metric}, $${p.defaults.valueUsd} per ${p.unit} per year; writes ${p
          .render(p.defaults)
          .map((f) => f.path)
          .join(', ')}`,
    ).join('\n') + '\n',
  );
  return 0;
}

/**
 * `ratchet init --pack <perf|forecasting>`: write ratchet.yaml, RATCHET.md and evaluator scaffolding into `dir`.
 * The generated ratchet.yaml is checked with the same parser as `ratchet validate` before anything is written.
 * Returns the process exit code: 0 written (or dry run), 1 refused or invalid.
 */
export function runInit(args: InitArgs, io: Io): number {
  if (args.listPacks) return listPacks(io);
  if (!args.pack) {
    io.err(`init: --pack is required (one of ${PACKS.map((p) => p.id).join(', ')}; see --list-packs)\n`);
    return 1;
  }
  const pack = findPack(args.pack);
  if (!pack) {
    io.err(`init: unknown pack ${JSON.stringify(args.pack)} (available: ${PACKS.map((p) => p.id).join(', ')})\n`);
    return 1;
  }

  const direction = args.direction ?? pack.defaults.direction;
  if (direction !== 'minimize' && direction !== 'maximize') {
    io.err(`init: --direction must be "minimize" or "maximize", got ${JSON.stringify(direction)}\n`);
    return 1;
  }
  const options: InitOptions = {
    metric: args.metric?.trim() || pack.defaults.metric,
    direction: direction as Direction,
    valueUsd: args.valueUsd ?? pack.defaults.valueUsd,
  };
  if (!Number.isFinite(options.valueUsd) || options.valueUsd <= 0) {
    io.err('init: --value-usd must be a positive number\n');
    return 1;
  }

  const files = pack.render(options);
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
  const lines = files.map((f: TemplateFile) => `  ${verb} ${f.path}${existing.includes(f) ? ' (overwritten)' : ''}  ${f.content.split('\n').length - 1} lines`);
  if (args.dryRun) {
    io.out(`ratchet init --pack ${pack.id} (dry run, nothing written to ${root})\n${lines.join('\n')}\n\n--- ratchet.yaml ---\n${generated.content}`);
    return 0;
  }
  for (const f of files) {
    const target = join(root, f.path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, f.content);
  }
  io.out(
    `ratchet init --pack ${pack.id}: ${options.direction} ${options.metric}, $${options.valueUsd} per ${pack.unit} per year (estimated)\n${lines.join('\n')}\n\n` +
      `Next: replace the TODOs, then \`ratchet validate\` and \`ratchet baseline\`.\n`,
  );
  return 0;
}
