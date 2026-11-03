import type { Command } from './args.js';
import { configCommand } from './config.js';
import { due } from './due.js';
import { exportCommand } from './export.js';
import { help } from './help.js';
import { list } from './list.js';
import { search } from './search.js';
import { show } from './show.js';
import { stats } from './stats.js';
import { version } from './version.js';

export const COMMANDS: Command[] = [configCommand, due, exportCommand, help, list, search, show, stats, version];
