import { t } from '../messages.js';
import { VERSION } from '../version.js';
import { parse, type Command } from './args.js';

export const version: Command = {
  name: 'version',
  run(ctx, args) {
    const { values } = parse(args, { options: { short: { type: 'boolean' }, verbose: { type: 'boolean' } } });
    if (values.short) {
      ctx.out(`${VERSION}\n`);
      return 0;
    }
    ctx.out(`taskr ${VERSION}\n`);
    if (values.verbose) {
      ctx.out(t('version.verbose', { sources: ctx.sources.length, plugins: ctx.plugins.length }) + '\n');
    }
    return 0;
  },
};
