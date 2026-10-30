/** The CLI invocations whose exact output is pinned in tests/golden/ (against the GOLDEN_SPEC workspace). */
export interface GoldenCase {
  name: string;
  args: string[];
  /** Expected exit code. */
  exit: number;
  /** Which stream is pinned (the other one must be empty). */
  stream: 'stdout' | 'stderr';
}

const out = (name: string, args: string[]): GoldenCase => ({ name, args, exit: 0, stream: 'stdout' });

export const GOLDEN_CASES: GoldenCase[] = [
  out('list-default', ['list']),
  out('list-filtered', ['list', '--status', 'doing', '--assignee', 'maya.chen', '--sort', 'priority', '--limit', '8']),
  out('list-label-due', ['list', '--label', 'billing', '--sort', 'due', '--limit', '12']),
  out('list-empty', ['list', '--assignee', 'nobody']),
  out('show', ['show', '17']),
  out('show-tricky', ['show', '97']),
  out('stats-status', ['stats']),
  out('stats-assignee', ['stats', '--by', 'assignee', '--top', '6']),
  out('stats-label', ['stats', '--by', 'label', '--top', '8']),
  out('stats-priority', ['stats', '--by', 'priority']),
  out('search', ['search', 'checkout', 'retry']),
  out('search-prefix', ['search', 'migr', '--limit', '5']),
  out('search-none', ['search', 'zzyzx']),
  out('export-csv', ['export', '--format', 'csv', '--limit', '110']),
  out('export-json', ['export', '--format', 'json', '--status', 'blocked', '--limit', '6']),
  out('export-md', ['export', '--format', 'md', '--limit', '200']),
  out('due-default', ['due']),
  out('due-tokyo', ['due', '--within', '30', '--tz', 'Asia/Tokyo']),
  out('due-utc', ['due', '--within', '0', '--tz', 'UTC']),
  out('config', ['config']),
  out('config-json', ['config', '--format', 'json']),
  out('help', ['help']),
  out('help-list', ['help', 'list']),
  out('version', ['version']),
  out('version-verbose', ['version', '--verbose']),
  { name: 'unknown-command', args: ['lst'], exit: 2, stream: 'stderr' },
  { name: 'unknown-far', args: ['frobnicate', '--now'], exit: 2, stream: 'stderr' },
  { name: 'error-no-task', args: ['show', '999999'], exit: 1, stream: 'stderr' },
  { name: 'error-bad-status', args: ['list', '--status', 'later'], exit: 1, stream: 'stderr' },
  { name: 'error-bad-zone', args: ['due', '--tz', 'Mars/Olympus'], exit: 1, stream: 'stderr' },
  { name: 'error-bad-option', args: ['list', '--colour'], exit: 1, stream: 'stderr' },
];
