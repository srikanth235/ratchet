import { parseArgs, type ParseArgsConfig } from 'node:util';
import type { Context } from '../context.js';
import { TaskrError } from '../errors.js';
import { STATUSES, type Status, type Task } from '../types.js';

export interface Command {
  name: string;
  run(ctx: Context, args: string[]): number | Promise<number>;
}

/** `util.parseArgs` in strict mode, with its failures reported as the catalog's E0202. */
export function parse<T extends ParseArgsConfig>(args: string[], config: T) {
  try {
    return parseArgs({ ...config, args, allowPositionals: config.allowPositionals ?? false, strict: true });
  } catch (err) {
    throw new TaskrError('E0202', { problem: (err as Error).message.split('\n')[0]! });
  }
}

export function intOption(name: string, value: string | undefined, fallback: number, min = 1): number {
  if (value === undefined) return fallback;
  const n = Number(value);
  if (!Number.isInteger(n) || n < min) throw new TaskrError('E0202', { problem: `--${name} must be an integer >= ${min}` });
  return n;
}

export function statusOption(value: string | undefined): Status | undefined {
  if (value === undefined) return undefined;
  if (!STATUSES.includes(value as Status)) throw new TaskrError('E0205', { value, allowed: STATUSES.join(', ') });
  return value as Status;
}

export interface TaskFilter {
  status?: Status | undefined;
  assignee?: string | undefined;
  label?: string | undefined;
}

/** Tasks matching every given filter, in id order, starting from the smallest index that applies. */
export function filterTasks(ctx: Context, f: TaskFilter): Task[] {
  const { store } = ctx;
  let pool: Task[] = store.tasks;
  if (f.assignee !== undefined) pool = store.byAssignee.get(f.assignee) ?? [];
  else if (f.label !== undefined) pool = store.byLabel.get(f.label) ?? [];
  else if (f.status !== undefined) pool = store.byStatus.get(f.status) ?? [];
  return pool.filter(
    (t) => (f.status === undefined || t.status === f.status) && (f.assignee === undefined || t.assignee === f.assignee) && (f.label === undefined || t.labels.includes(f.label)),
  );
}
