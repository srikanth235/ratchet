import { STATUSES, type Status, type Task } from './types.js';

const DATE = /^\d{4}-\d{2}-\d{2}$/;
const LABEL = /^[a-z][a-z0-9-]{0,23}$/;
const ASSIGNEE = /^[a-z][a-z0-9._-]{0,31}$/;

export class ValidationError extends Error {
  constructor(
    readonly index: number,
    readonly field: string,
    problem: string,
  ) {
    super(`task #${index}: ${field} ${problem}`);
  }
}

function isoDate(index: number, field: string, v: unknown): string {
  if (typeof v !== 'string' || !DATE.test(v) || Number.isNaN(Date.parse(`${v}T00:00:00Z`))) throw new ValidationError(index, field, 'must be an ISO date (YYYY-MM-DD)');
  return v;
}

/** Check one raw record of the task file and return it as a `Task`. */
export function validateTask(raw: unknown, index: number): Task {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new ValidationError(index, 'record', 'must be an object');
  const r = raw as Record<string, unknown>;
  if (!Number.isInteger(r.id) || (r.id as number) < 1) throw new ValidationError(index, 'id', 'must be a positive integer');
  if (typeof r.title !== 'string' || r.title.trim() === '' || r.title.length > 200) throw new ValidationError(index, 'title', 'must be 1 to 200 characters');
  if (!STATUSES.includes(r.status as Status)) throw new ValidationError(index, 'status', `must be one of ${STATUSES.join(', ')}`);
  if (![1, 2, 3, 4].includes(r.priority as number)) throw new ValidationError(index, 'priority', 'must be 1 to 4');
  if (typeof r.assignee !== 'string' || !ASSIGNEE.test(r.assignee)) throw new ValidationError(index, 'assignee', 'must be a lowercase handle');
  if (!Array.isArray(r.labels) || r.labels.some((l) => typeof l !== 'string' || !LABEL.test(l))) throw new ValidationError(index, 'labels', 'must be lowercase slugs');
  if (typeof r.estimate !== 'number' || !(r.estimate >= 0) || r.estimate > 1000) throw new ValidationError(index, 'estimate', 'must be 0 to 1000 hours');
  const created = isoDate(index, 'created', r.created);
  const due = r.due === null ? null : isoDate(index, 'due', r.due);
  return {
    id: r.id as number,
    title: r.title,
    status: r.status as Status,
    priority: r.priority as 1 | 2 | 3 | 4,
    assignee: r.assignee,
    labels: r.labels as string[],
    estimate: r.estimate,
    created,
    due,
  };
}
