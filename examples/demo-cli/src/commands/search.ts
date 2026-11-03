import { suggest } from '../dictionary.js';
import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { renderTable } from '../render/table.js';
import { tokenize } from '../store.js';
import { intOption, parse, type Command } from './args.js';

export const search: Command = {
  name: 'search',
  run(ctx, args) {
    const { values, positionals } = parse(args, { options: { limit: { type: 'string' } }, allowPositionals: true });
    const terms = tokenize(positionals.join(' '));
    if (terms.length === 0) throw new TaskrError('E0203', { what: 'a search term' });
    const limit = intOption('limit', values.limit as string | undefined, 10);

    // every term must be a prefix of some title word; a task matches when each term hits one of its words
    const index = ctx.store.titleIndex;
    let ids: Set<number> | undefined;
    for (const term of terms) {
      const hit = new Set<number>();
      for (const [word, list] of index) if (word.startsWith(term)) for (const id of list) hit.add(id);
      ids = ids === undefined ? hit : new Set([...ids].filter((id) => hit.has(id)));
    }
    const found = [...(ids ?? [])].map((id) => ctx.store.byId.get(id)!).sort((a, b) => a.priority - b.priority || a.id - b.id);
    if (found.length === 0) {
      ctx.out(t('search.none', { query: terms.join(' ') }) + '\n');
      const alternatives = suggest(terms[terms.length - 1]!);
      if (alternatives.length) ctx.out(t('search.suggest', { words: alternatives.join(', ') }) + '\n');
      return 0;
    }
    const shown = found.slice(0, limit);
    ctx.out(
      renderTable(
        [{ title: t('list.header.id'), align: 'right' }, { title: t('list.header.priority') }, { title: t('list.header.assignee') }, { title: t('list.header.title'), flex: true }],
        shown.map((x) => [String(x.id), `P${x.priority}`, x.assignee, x.title]),
        ctx.terminal.columns,
      ),
    );
    ctx.out(t('search.summary', { shown: shown.length, total: found.length }) + '\n');
    return 0;
  },
};
