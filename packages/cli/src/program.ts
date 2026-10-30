import { Command, CommanderError, InvalidArgumentError } from 'commander';
import { runBaseline } from './commands/baseline.js';
import { runInit } from './commands/init.js';
import { runValidate } from './commands/validate.js';
import type { Io } from './io.js';

function parseNumber(value: string): number {
  const n = Number(value);
  if (value.trim() === '' || !Number.isFinite(n)) throw new InvalidArgumentError('must be a number');
  return n;
}

function parsePositiveInt(value: string): number {
  const n = parseNumber(value);
  if (!Number.isInteger(n) || n < 1) throw new InvalidArgumentError('must be a positive integer');
  return n;
}

/** Build the commander program. Commands record their exit code via `setExit`. */
export function buildProgram(io: Io, setExit: (code: number) => void): Command {
  const program = new Command('ratchet')
    .description('Ratchet: autonomous optimization of a repo against a benchmark')
    .exitOverride()
    .configureOutput({ writeOut: (s) => io.out(s), writeErr: (s) => io.err(s) });

  program
    .command('validate')
    .description('validate ratchet.yaml and print a summary')
    .argument('[path]', 'path to ratchet.yaml', 'ratchet.yaml')
    .action((path: string) => {
      setExit(runValidate(path, io));
    });

  program
    .command('init')
    .description(
      'scaffold ratchet.yaml, RATCHET.md and an evaluator stub for a domain pack.\n' +
        'Refuses to overwrite existing files unless --force is given.',
    )
    .option('--pack <pack>', 'domain pack: perf | forecasting (see --list-packs)')
    .option('--metric <path>', "objective metric, a dotted path into the evaluator JSON (default: the pack's)")
    .option('--direction <dir>', "minimize | maximize (default: the pack's)")
    .option('--value-usd <usd>', 'estimated USD per unit of improvement per year', parseNumber)
    .option('--dir <dir>', 'directory to write into', '.')
    .option('--force', 'overwrite existing files')
    .option('--dry-run', 'print what would be written, write nothing')
    .option('--list-packs', 'list the available packs and exit')
    .action(
      (o: { pack?: string; metric?: string; direction?: string; valueUsd?: number; dir?: string; force?: boolean; dryRun?: boolean; listPacks?: boolean }) => {
        setExit(runInit(o, io));
      },
    );

  program
    .command('baseline')
    .description(
      'run bench.run locally N times and print stats and the noise floor.\n' +
        'The bench command must print JSON on stdout; the last JSON object is used and\n' +
        'objective.metric is looked up in it by dotted path (e.g. http_req_duration.p95).',
    )
    .argument('[path]', 'path to ratchet.yaml', 'ratchet.yaml')
    .option('-n, --repeats <n>', 'number of runs (default: bench.repeats)', parsePositiveInt)
    .option('--json', 'machine-readable JSON output')
    .option('--k <k>', 'noise floor = max(k x stddev, min-absolute); default 2', parseNumber)
    .option('--min-absolute <x>', 'absolute lower bound for the noise floor; default 0', parseNumber)
    .option('--timeout <ms>', 'kill a single bench run after this many ms', parsePositiveInt)
    .action(
      async (
        path: string,
        o: { repeats?: number; json?: boolean; k?: number; minAbsolute?: number; timeout?: number },
      ) => {
        setExit(
          await runBaseline(
            path,
            { repeats: o.repeats, json: o.json, k: o.k, minAbsolute: o.minAbsolute, timeoutMs: o.timeout },
            io,
          ),
        );
      },
    );

  return program;
}

/** Run the CLI with `argv` (without node/script, i.e. `process.argv.slice(2)`). Returns the exit code. */
export async function runCli(argv: string[], io: Io): Promise<number> {
  let code = 0;
  const program = buildProgram(io, (c) => {
    code = c;
  });
  for (const sub of program.commands) sub.exitOverride();
  try {
    await program.parseAsync(argv, { from: 'user' });
  } catch (err) {
    if (err instanceof CommanderError) return err.exitCode;
    io.err(`${(err as Error).message}\n`);
    return 1;
  }
  return code;
}
