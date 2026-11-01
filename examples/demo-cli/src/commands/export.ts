import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { renderCsv } from '../render/csv.js';
import { escapeProse, renderMarkdownTable } from '../render/markdown.js';
import { filterTasks, intOption, parse, statusOption, type Command } from './args.js';

const FORMATS = ['json', 'csv', 'md'];
const COLUMNS = ['id', 'title', 'status', 'priority', 'assignee', 'labels', 'estimate', 'created', 'due'];

export const exportCommand: Command = {
  name: 'export',
  run(ctx, args) {
    const { values } = parse(args, {
      options: { format: { type: 'string', default: 'json' }, status: { type: 'string' }, assignee: { type: 'string' }, limit: { type: 'string' } },
    });
    const format = values.format as string;
    if (!FORMATS.includes(format)) throw new TaskrError('E0204', { format, allowed: FORMATS.join(', ') });
    const status = statusOption(values.status as string | undefined);
    const tasks = filterTasks(ctx, { status, assignee: values.assignee as string | undefined });
    const limit = intOption('limit', values.limit as string | undefined, tasks.length || 1);
    const rows = tasks.slice(0, limit);

    if (format === 'json') {
      ctx.out(JSON.stringify(rows, null, 2) + '\n');
    } else if (format === 'csv') {
      ctx.out(renderCsv(COLUMNS, rows.map((x) => [x.id, x.title, x.status, x.priority, x.assignee, x.labels.join(';'), x.estimate, x.created, x.due])));
    } else {
      ctx.out(`# ${escapeProse(t('export.md.title', { count: rows.length }))}\n\n`);
      ctx.out(renderMarkdownTable(COLUMNS, rows.map((x) => [String(x.id), x.title, x.status, String(x.priority), x.assignee, x.labels.join(', '), String(x.estimate), x.created, x.due ?? ''])));
    }
    return 0;
  },
};
