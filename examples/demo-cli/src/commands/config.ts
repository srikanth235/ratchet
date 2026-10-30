import { relative } from 'node:path';
import { t } from '../messages.js';
import { formatOffset, zoneInfo } from '../tz.js';
import { parse, type Command } from './args.js';
import { TaskrError } from '../errors.js';

export const configCommand: Command = {
  name: 'config',
  run(ctx, args) {
    const { values } = parse(args, { options: { format: { type: 'string', default: 'text' } } });
    const rel = (file: string) => relative(ctx.rootDir, file) || '.';
    const sources = ctx.sources.map((s) => rel(s.file));
    const info = zoneInfo(ctx.config.tz);
    if (values.format === 'json') {
      ctx.out(
        JSON.stringify(
          { root: '.', sources, dataFile: rel(ctx.config.dataFile), tz: ctx.config.tz, pageSize: ctx.config.pageSize, columns: ctx.terminal.columns, plugins: ctx.plugins },
          null,
          2,
        ) + '\n',
      );
    } else if (values.format === 'text') {
      const row = (key: string, value: string) => ctx.out(`${t(`config.${key}`).padEnd(12)}${value}\n`);
      row('sources', sources.length ? sources.join(', ') : t('config.none'));
      row('dataFile', rel(ctx.config.dataFile));
      row('tz', `${ctx.config.tz} (${formatOffset(info.standard)})`);
      row('pageSize', String(ctx.config.pageSize));
      row('terminal', t('config.terminalValue', { columns: ctx.terminal.columns, colors: ctx.terminal.colors }));
      row('plugins', ctx.plugins.length ? ctx.plugins.map((p) => `${p.name}@${p.version}`).join(', ') : t('config.none'));
    } else {
      throw new TaskrError('E0204', { format: String(values.format), allowed: 'text, json' });
    }
    return 0;
  },
};
