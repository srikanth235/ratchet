import { TaskrError } from '../errors.js';
import { t } from '../messages.js';
import { formatOffset, isKnownZone, localDate, zoneInfo } from '../tz.js';
import { intOption, parse, type Command } from './args.js';

const DAY = 86_400_000;

export const due: Command = {
  name: 'due',
  run(ctx, args) {
    const { values } = parse(args, { options: { within: { type: 'string' }, tz: { type: 'string' } } });
    const within = intOption('within', values.within as string | undefined, 14, 0);
    const zone = (values.tz as string | undefined) ?? ctx.config.tz;
    if (!isKnownZone(zone)) throw new TaskrError('E0101', { tz: zone });
    const info = zoneInfo(zone);

    const today = localDate(ctx.now.getTime(), zone);
    const last = localDate(ctx.now.getTime() + within * DAY, zone);
    const open = ctx.store.tasks.filter((x) => x.status !== 'done' && x.due !== null);
    const overdue = open.filter((x) => x.due! < today).length;
    const upcoming = open.filter((x) => x.due! >= today && x.due! <= last).sort((a, b) => a.due!.localeCompare(b.due!) || a.priority - b.priority || a.id - b.id);

    const offset = info.daylight === null ? formatOffset(info.standard) : `${formatOffset(info.standard)}/${formatOffset(info.daylight)}`;
    ctx.out(t('due.header', { within, zone, offset, today }) + '\n');
    let day = '';
    for (const x of upcoming) {
      if (x.due !== day) {
        day = x.due!;
        ctx.out(`${day}\n`);
      }
      ctx.out(`  #${x.id} P${x.priority} ${x.assignee.padEnd(14)} ${x.title}\n`);
    }
    ctx.out(t('due.summary', { count: upcoming.length, overdue }) + '\n');
    return 0;
  },
};
