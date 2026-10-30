import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { renderTable } from '../render/table.js';
import type { Task } from '../types.js';
import { filterTasks, intOption, parse, statusOption, type Command } from './args.js';

const SORTS: Record<string, (a: Task, b: Task) => number> = {
  id: (a, b) => a.id - b.id,
  priority: (a, b) => a.priority - b.priority || a.id - b.id,
  due: (a, b) => (a.due ?? '9999-12-31').localeCompare(b.due ?? '9999-12-31') || a.id - b.id,
  estimate: (a, b) => b.estimate - a.estimate || a.id - b.id,
};

export const list: Command = {
  name: 'list',
  run(ctx, args) {
    const { values } = parse(args, {
      options: {
        status: { type: 'string' },
        assignee: { type: 'string' },
        label: { type: 'string' },
        sort: { type: 'string', default: 'id' },
        limit: { type: 'string' },
      },
    });
    const sort = SORTS[values.sort as string];
    if (!sort) throw new TaskrError('E0202', { problem: `--sort must be one of ${Object.keys(SORTS).join(', ')}` });
    const limit = intOption('limit', values.limit as string | undefined, ctx.config.pageSize);
    const matches = filterTasks(ctx, {
      status: statusOption(values.status as string | undefined),
      assignee: values.assignee as string | undefined,
      label: values.label as string | undefined,
    }).sort(sort);
    const shown = matches.slice(0, limit);
    if (shown.length === 0) {
      ctx.out(t('list.empty') + '\n');
      return 0;
    }
    ctx.out(
      renderTable(
        [
          { title: t('list.header.id'), align: 'right' },
          { title: t('list.header.priority') },
          { title: t('list.header.status') },
          { title: t('list.header.assignee') },
          { title: t('list.header.due') },
          { title: t('list.header.title'), flex: true },
        ],
        shown.map((x) => [String(x.id), `P${x.priority}`, x.status, x.assignee, x.due ?? '-', x.title]),
        ctx.terminal.columns,
      ),
    );
    ctx.out(t('list.summary', { shown: shown.length, total: matches.length }) + '\n');
    return 0;
  },
};
