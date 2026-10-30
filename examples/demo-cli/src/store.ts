import { existsSync, readFileSync } from 'node:fs';
import type { Status, Task } from './types.js';
import { validateTask } from './validate.js';

export interface Store {
  tasks: Task[];
  byId: Map<number, Task>;
  byAssignee: Map<string, Task[]>;
  byLabel: Map<string, Task[]>;
  byStatus: Map<Status, Task[]>;
  /** Lowercased title word -> ids of the tasks containing it. */
  titleIndex: Map<string, number[]>;
}

function push<K, V>(map: Map<K, V[]>, key: K, value: V): void {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

export function tokenize(text: string): string[] {
  return text.toLowerCase().match(/[a-z0-9]+/g) ?? [];
}

/** Read the task file, validate every record and build the lookup indexes. A missing file is an empty store. */
export function loadStore(file: string): Store {
  const store: Store = {
    tasks: [],
    byId: new Map(),
    byAssignee: new Map(),
    byLabel: new Map(),
    byStatus: new Map(),
    titleIndex: new Map(),
  };
  if (!existsSync(file)) return store;
  const raw = JSON.parse(readFileSync(file, 'utf8')) as unknown;
  if (!Array.isArray(raw)) throw new Error(`${file}: expected a JSON array of tasks`);
  raw.forEach((r, i) => store.tasks.push(validateTask(r, i)));
  for (const task of store.tasks) {
    store.byId.set(task.id, task);
    push(store.byAssignee, task.assignee, task);
    push(store.byStatus, task.status, task);
    for (const label of task.labels) push(store.byLabel, label, task);
    for (const word of new Set(tokenize(task.title))) push(store.titleIndex, word, task.id);
  }
  return store;
}
