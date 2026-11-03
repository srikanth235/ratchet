// Regenerates data/messages/*.json (deterministic):  node data/gen-messages.mjs
//
// Each catalog is a flat JSON object of ICU-style messages. The areas the commands use (cli, list, show, stats,
// search, export, due, config, help, version) are small and written by hand below; `errors`, `validation`,
// `deprecations`, `wizard`, `telemetry` and `plugins` are the long tail every mature CLI accumulates (an error
// catalog with hundreds of codes, a message for every validation rule of every field, ...). Output of the real
// commands never changes when the long tail does.
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const out = (name) => fileURLToPath(new URL(`./messages/${name}.json`, import.meta.url));
mkdirSync(fileURLToPath(new URL('./messages/', import.meta.url)), { recursive: true });

function mulberry32(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rng = mulberry32(7);
const pick = (a) => a[Math.floor(rng() * a.length)];
const write = (name, obj) => writeFileSync(out(name), `${JSON.stringify(obj, null, 1)}\n`);

// ---- hand-written areas ----------------------------------------------------------------------------------
const cmds = {
  config: ['taskr config [--format text|json]', 'Show the configuration, task file, terminal settings and plugins that apply here.'],
  due: ['taskr due [--within DAYS] [--tz ZONE]', 'List open tasks due within the next DAYS days (default 14), grouped by day in the given IANA time zone.'],
  export: ['taskr export [--format json|csv|md] [--status S] [--assignee A] [--limit N]', 'Write the matching tasks to stdout as JSON, CSV or a Markdown table.'],
  help: ['taskr help [command]', 'Show the command overview, or the usage of one command.'],
  list: ['taskr list [--status S] [--assignee A] [--label L] [--sort id|priority|due|estimate] [--limit N]', 'List tasks as a table, filtered and sorted. The title column is truncated to the terminal width.'],
  search: ['taskr search [--limit N] WORDS...', 'Find tasks whose titles contain every word (as a word prefix). Suggests spellings when nothing matches.'],
  show: ['taskr show ID', 'Show every field of one task.'],
  stats: ['taskr stats [--by status|assignee|label|priority] [--top N]', 'Count tasks and estimated hours per group, with a bar chart.'],
  version: ['taskr version [--short] [--verbose]', 'Print the version.'],
};
const summaries = {
  config: 'show the effective configuration',
  due: 'tasks due soon, by day',
  export: 'write tasks as JSON, CSV or Markdown',
  help: 'show help',
  list: 'list tasks',
  search: 'find tasks by title',
  show: 'show one task',
  stats: 'counts and hours per group',
  version: 'print the version',
};
const core = {
  'cli.unknown': 'Unknown command "{name}".',
  'cli.didYouMean': 'Did you mean: {names}?',
  'list.empty': 'No tasks match.',
  'list.header.id': 'ID',
  'list.header.priority': 'PRI',
  'list.header.status': 'STATUS',
  'list.header.assignee': 'ASSIGNEE',
  'list.header.due': 'DUE',
  'list.header.title': 'TITLE',
  'list.summary': '{shown} of {total, plural, one {# task} other {# tasks}}',
  'show.status': 'status',
  'show.priority': 'priority',
  'show.assignee': 'assignee',
  'show.labels': 'labels',
  'show.estimate': 'estimate',
  'show.created': 'created',
  'show.due': 'due',
  'show.hours': '{n, plural, one {# hour} other {# hours}}',
  'stats.by.status': 'STATUS',
  'stats.by.assignee': 'ASSIGNEE',
  'stats.by.label': 'LABEL',
  'stats.by.priority': 'PRIORITY',
  'stats.header.count': 'TASKS',
  'stats.header.hours': 'HOURS',
  'stats.header.share': 'SHARE',
  'stats.total': '{total, plural, one {# task} other {# tasks}} in {groups, plural, one {# group} other {# groups}}',
  'search.none': 'No task title contains "{query}".',
  'search.suggest': 'Try: {words}',
  'search.summary': '{shown} of {total, plural, one {# match} other {# matches}}',
  'export.md.title': 'Tasks ({count, plural, one {# task} other {# tasks}})',
  'due.header': 'Due within {within, plural, one {# day} other {# days}} in {zone} ({offset}); today is {today}',
  'due.summary': '{count, plural, one {# task} other {# tasks}} due, {overdue} overdue',
  'config.sources': 'sources',
  'config.dataFile': 'data file',
  'config.tz': 'time zone',
  'config.pageSize': 'page size',
  'config.terminal': 'terminal',
  'config.plugins': 'plugins',
  'config.none': '(none)',
  'config.terminalValue': '{columns} columns, {colors} colors',
  'help.title': 'taskr {version}: a small task tracker',
  'help.usage': 'Usage: taskr <command> [options]',
  'help.footer': 'Run "taskr help <command>" for the options of one command.',
  'version.verbose': 'config files: {sources}, plugins: {plugins}',
};
for (const [c, [usage, long]] of Object.entries(cmds)) {
  core[`help.${c}.usage`] = `Usage: ${usage}`;
  core[`help.${c}.long`] = long;
  core[`help.${c}.summary`] = summaries[c];
}
const byArea = {};
for (const [k, v] of Object.entries(core)) (byArea[k.split('.')[0]] ??= {})[k] = v;
for (const [area, obj] of Object.entries(byArea)) write(area, obj);

// ---- the long tail ----------------------------------------------------------------------------------------
const prefix = (p, o) => Object.fromEntries(Object.entries(o).map(([k, v]) => [`${p}.${k}`, v]));
write('errors', prefix('errors', {
  E0101: 'Unknown time zone {tz}. Use an IANA name such as Europe/Berlin.',
  E0201: 'No task with id {id}.',
  E0202: 'Invalid option: {problem}.',
  E0203: 'Missing {what}.',
  E0204: 'Unsupported format {format}; use one of: {allowed}.',
  E0205: 'Unknown status {value}; use one of: {allowed}.',
  E0206: 'No help for "{name}".',
  ...(() => {
    const verbs = ['read', 'write', 'parse', 'merge', 'archive', 'restore', 'assign', 'reopen', 'index', 'lock', 'sync', 'rename', 'move', 'link', 'split', 'export', 'import', 'schedule'];
    const nouns = ['task', 'label', 'milestone', 'board', 'sprint', 'comment', 'attachment', 'workspace', 'filter', 'webhook', 'template', 'checklist', 'reminder', 'epic'];
    const reasons = [
      'the {noun} is locked by {owner}',
      'the file {path} is not writable',
      '{count, plural, one {# dependency is} other {# dependencies are}} still open',
      'the {kind, select, remote {remote} local {local} other {cached}} copy is out of date',
      'it exceeds the limit of {limit} per {scope}',
      'the {noun} was deleted by {owner}',
      'the schema version {found} is newer than {supported}',
      'the request timed out after {seconds, plural, one {# second} other {# seconds}}',
    ];
    const o = {};
    for (let i = 300; i < 1300; i++) {
      const noun = pick(nouns);
      o[`E${String(i).padStart(4, '0')}`] = `Cannot ${pick(verbs)} ${noun} {id}: ${pick(reasons).replace('{noun}', noun)}.`;
    }
    return o;
  })(),
}));
const fields = ['title', 'status', 'priority', 'assignee', 'labels', 'estimate', 'created', 'due', 'parent', 'sprint', 'milestone', 'reporter', 'component', 'severity', 'environment', 'version', 'resolution', 'watchers', 'points', 'rank'];
const rules = [
  ['required', '{field} is required'],
  ['type', '{field} must be {kind, select, string {a string} number {a number} date {an ISO date} other {a valid value}}'],
  ['min', '{field} must be at least {min}'],
  ['max', '{field} must be at most {max}'],
  ['length', '{field} must be {count, plural, =0 {empty} one {exactly # character} other {exactly # characters}} long'],
  ['pattern', '{field} does not match the pattern {pattern}'],
  ['enum', '{field} must be one of: {allowed}'],
  ['unique', '{field} {value} is already used by task {other}'],
  ['future', '{field} must not be in the past'],
  ['order', '{field} must not be earlier than {other}'],
];
const validation = {};
for (const f of fields) for (const [rule, text] of rules) for (const variant of ['record', 'import', 'api', 'bulk', 'cli', 'webhook']) {
  validation[`validation.${f}.${rule}.${variant}`] = `${variant === 'record' ? '' : `[${variant}] `}${text.replace('{field}', f)}`;
}
write('validation', validation);
const dep = {};
const flags = ['--json', '--csv', '--all', '--mine', '--open', '--closed', '--tag', '--owner', '--since', '--until', '--sort', '--reverse', '--format', '--page', '--no-color', '--wide', '--quiet', '--verbose', '--dry-run', '--force'];
for (let i = 0; i < 600; i++) {
  const f = flags[i % flags.length];
  dep[`deprecations.D${String(i).padStart(4, '0')}`] = `${f}${i > 20 ? `-v${Math.floor(i / 20)}` : ''} is deprecated since {since} and is removed in {removal}; use {replacement} instead.`;
}
write('deprecations', dep);
const wiz = {};
const steps = ['welcome', 'workspace', 'import', 'team', 'labels', 'sprints', 'integrations', 'notifications', 'review', 'finish'];
for (const s of steps) for (let i = 0; i < 60; i++) {
  wiz[`wizard.${s}.q${i}`] = pick([`Which {thing} should ${s} use?`, `How many {thing, plural, one {# {unit}} other {# {unit}s}} per ${s}?`, `Enable {feature} for ${s}? ({default, select, yes {Y/n} other {y/N}})`, `Press enter to keep {value} for ${s}.`]);
}
write('wizard', wiz);
const tel = {};
for (let i = 0; i < 400; i++) tel[`telemetry.event${i}`] = `${pick(['command', 'option', 'plugin', 'error', 'timing'])} ${pick(['started', 'finished', 'failed', 'skipped'])} after {ms}ms (run {run}, {count, plural, one {# retry} other {# retries}})`;
write('telemetry', tel);
const plug = {};
for (let i = 0; i < 400; i++) plug[`plugins.P${String(i).padStart(4, '0')}`] = `Plugin {name}@{version} ${pick(['requires', 'conflicts with', 'replaces', 'extends'])} {other} ${pick(['>= 1.2', '< 2.0', '(any version)', 'in this workspace'])}.`;
write('plugins', plug);
console.log('wrote data/messages/*.json');
