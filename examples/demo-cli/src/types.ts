export const STATUSES = ['todo', 'doing', 'blocked', 'done'] as const;
export type Status = (typeof STATUSES)[number];

export interface Task {
  id: number;
  title: string;
  status: Status;
  /** 1 (urgent) to 4 (someday). */
  priority: 1 | 2 | 3 | 4;
  assignee: string;
  labels: string[];
  /** Estimate in hours. */
  estimate: number;
  /** ISO date (YYYY-MM-DD). */
  created: string;
  /** ISO date or null. */
  due: string | null;
}

export interface Config {
  /** Stop the upward config search here. */
  root: boolean;
  /** Task file, relative to the directory of the config that sets it. */
  dataFile: string;
  /** IANA time zone used to show dates. */
  tz: string;
  /** Default row limit of `list`. */
  pageSize: number;
}

export const DEFAULT_CONFIG: Config = { root: false, dataFile: 'tasks.json', tz: 'UTC', pageSize: 20 };

export interface Plugin {
  name: string;
  version: string;
}
