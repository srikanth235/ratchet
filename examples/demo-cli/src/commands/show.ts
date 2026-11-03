import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { parse, type Command } from './args.js';

const PRIORITY_NAMES = ['', 'urgent', 'high', 'normal', 'someday'];

export const show: Command = {
  name: 'show',
  run(ctx, args) {
    const { positionals } = parse(args, { options: {}, allowPositionals: true });
    const raw = positionals[0];
    if (raw === undefined) throw new TaskrError('E0203', { what: 'a task id' });
    const id = Number(raw);
    const task = Number.isInteger(id) ? ctx.store.byId.get(id) : undefined;
    if (!task) throw new TaskrError('E0201', { id: raw });
    const row = (key: string, value: string) => `  ${t(`show.${key}`).padEnd(10)}${value}\n`;
    ctx.out(`#${task.id} ${task.title}\n`);
    ctx.out(row('status', task.status));
    ctx.out(row('priority', `P${task.priority} (${PRIORITY_NAMES[task.priority]})`));
    ctx.out(row('assignee', task.assignee));
    ctx.out(row('labels', task.labels.length ? task.labels.join(', ') : '-'));
    ctx.out(row('estimate', t('show.hours', { n: task.estimate })));
    ctx.out(row('created', task.created));
    ctx.out(row('due', task.due ? `${task.due} (${ctx.config.tz})` : '-'));
    return 0;
  },
};
