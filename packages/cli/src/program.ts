import { Command, CommanderError, InvalidArgumentError } from 'commander';
import { runBaseline } from './commands/baseline.js';
import { runCostReconcile, type CostDeps } from './commands/cost.js';
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
export function buildProgram(io: Io, setExit: (code: number) => void, deps: CostDeps = {}): Command {
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

  const cost = program.command('cost').description('spend and billing tools');
  cost
    .command('reconcile')
    .description(
      'compare the ledger\'s estimated spend with AWS Cost Explorer (tag ratchet:opt) and record\n' +
        'correction rows. Idempotent. Prints "pending" and writes nothing while billing data is\n' +
        'incomplete (Cost Explorer lags up to 24 h). Needs ce:GetCostAndUsage.',
    )
    .argument('<optimizationId>', 'optimization to reconcile')
    .option('--db <path>', 'ledger SQLite file (default: $RATCHET_DB)')
    .option('--json', 'machine-readable JSON output')
    .option('--lag-hours <h>', 'hours after the optimization ended before billing is trusted; default 24', parseNumber)
    .option('--accept-estimated', 'treat days Cost Explorer still marks as estimated as final')
    .action(async (optimizationId: string, o: { db?: string; json?: boolean; lagHours?: number; acceptEstimated?: boolean }) => {
      setExit(await runCostReconcile(optimizationId, o, io, deps));
    });

  return program;
}

/** Run the CLI with `argv` (without node/script, i.e. `process.argv.slice(2)`). Returns the exit code. */
export async function runCli(argv: string[], io: Io, deps: CostDeps = {}): Promise<number> {
  let code = 0;
  const program = buildProgram(
    io,
    (c) => {
      code = c;
    },
    deps,
  );
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
