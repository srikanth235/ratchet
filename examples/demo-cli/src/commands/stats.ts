import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { bar, renderTable } from '../render/table.js';
import type { Task } from '../types.js';
import { parse, type Command } from './args.js';

const GROUPS: Record<string, (task: Task) => string[]> = {
  status: (x) => [x.status],
  assignee: (x) => [x.assignee],
  label: (x) => (x.labels.length ? x.labels : ['(none)']),
  priority: (x) => [`P${x.priority}`],
};

export const stats: Command = {
  name: 'stats',
  run(ctx, args) {
    const { values } = parse(args, { options: { by: { type: 'string', default: 'status' }, top: { type: 'string', default: '12' } } });
    const group = GROUPS[values.by as string];
    if (!group) throw new TaskrError('E0202', { problem: `--by must be one of ${Object.keys(GROUPS).join(', ')}` });
    const top = Number(values.top);
    if (!Number.isInteger(top) || top < 1) throw new TaskrError('E0202', { problem: '--top must be a positive integer' });

    const buckets = new Map<string, { count: number; hours: number }>();
    for (const task of ctx.store.tasks) {
      for (const key of group(task)) {
        const b = buckets.get(key) ?? { count: 0, hours: 0 };
        b.count++;
        b.hours += task.estimate;
        buckets.set(key, b);
      }
    }
    const sorted = [...buckets].sort((a, b) => b[1].count - a[1].count || (a[0] < b[0] ? -1 : 1)).slice(0, top);
    const max = sorted[0]?.[1].count ?? 0;
    const total = ctx.store.tasks.length;
    ctx.out(
      renderTable(
        [
          { title: t(`stats.by.${values.by as string}`) },
          { title: t('stats.header.count'), align: 'right' },
          { title: t('stats.header.hours'), align: 'right' },
          { title: t('stats.header.share'), align: 'right' },
          { title: '' },
        ],
        sorted.map(([key, b]) => [key, String(b.count), b.hours.toFixed(1), `${((b.count / total) * 100).toFixed(1)}%`, bar(b.count, max, 20, ctx.terminal.colors >= 8)]),
        ctx.terminal.columns,
      ),
    );
    ctx.out(t('stats.total', { total, groups: buckets.size }) + '\n');
    return 0;
  },
};
