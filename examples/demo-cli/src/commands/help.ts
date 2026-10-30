import { COMMAND_NAMES } from '../dictionary.js';
import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { VERSION } from '../version.js';
import { parse, type Command } from './args.js';

export const help: Command = {
  name: 'help',
  run(ctx, args) {
    const { positionals } = parse(args, { options: {}, allowPositionals: true });
    const topic = positionals[0];
    if (topic !== undefined) {
      if (!(COMMAND_NAMES as readonly string[]).includes(topic)) throw new TaskrError('E0206', { name: topic });
      ctx.out(`${t(`help.${topic}.usage`)}\n\n${t(`help.${topic}.long`)}\n`);
      return 0;
    }
    ctx.out(`${t('help.title', { version: VERSION })}\n\n${t('help.usage')}\n\n`);
    for (const name of COMMAND_NAMES) ctx.out(`  ${name.padEnd(9)}${t(`help.${name}.summary`)}\n`);
    ctx.out(`\n${t('help.footer')}\n`);
    return 0;
  },
};
