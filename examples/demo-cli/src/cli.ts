import { COMMANDS } from './commands/index.js';
import { createContext, type Io } from './context.js';
import { COMMAND_NAMES, suggest } from './dictionary.js';
import { TaskrError } from './errors.js';
import { t } from './messages.js';
import { maybeCheckForUpdates } from './update-check.js';
import { VERSION } from './version.js';

const ALIASES: Record<string, string> = { '--help': 'help', '-h': 'help', '--version': 'version', '-v': 'version' };

/** Run the CLI: returns the process exit code. `argv` is `process.argv.slice(2)`. */
export async function main(argv: string[], io: Io, cwd: string, env: NodeJS.ProcessEnv): Promise<number> {
  try {
    const first = argv[0] ?? 'help';
    const name = ALIASES[first] ?? first;
    const ctx = createContext(io, cwd, env);
    maybeCheckForUpdates(ctx, VERSION);
    const command = COMMANDS.find((c) => c.name === name);
    if (!command) {
      const near = suggest(name, 3, COMMAND_NAMES);
      io.err(`${t('cli.unknown', { name })}${near.length ? ` ${t('cli.didYouMean', { names: near.join(', ') })}` : ''}\n`);
      return 2;
    }
    return await command.run(ctx, argv.slice(1));
  } catch (err) {
    if (err instanceof TaskrError) {
      io.err(`taskr: error ${err.code}: ${err.message}\n`);
      return err.exitCode;
    }
    throw err;
  }
}
