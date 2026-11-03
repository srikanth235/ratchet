/**
 * The benchmark's workspace: a monorepo-shaped directory tree the CLI is run in. It is generated, not committed,
 * and identical for a given spec (seeded), so the visible mix, the hidden mix and the golden tests all see the
 * same "customer" and differ only in what is asked of the CLI.
 *
 *   <root>/outer/.taskrrc.json            decoy above the workspace root: must never apply
 *   <root>/outer/node_modules/...         ~200 packages
 *   <root>/outer/ws/.taskrrc.json         { "root": true, ... }  the config that applies
 *   <root>/outer/ws/data/tasks.json       the tasks
 *   <root>/outer/ws/node_modules/...      packages (two of them taskr plugins)
 *   <root>/outer/ws/packages/web/{package.json,node_modules}
 *   <root>/outer/ws/packages/web/src/components/forms    the working directory of every invocation
 */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRng, pick } from '../src/rng.js';

export const NOW = '2026-03-02T09:30:00Z';

export const ASSIGNEES = ['maya.chen', 'tomas.ruiz', 'priya.nair', 'jonas.berg', 'amara.okafor', 'lena.fischer', 'diego.alves', 'sofia.rossi', 'kenji.mori', 'hana.kim', 'omar.haddad', 'nora.lund', 'ivan.petrov', 'zoe.martin', 'raj.patel', 'ella.novak', 'sam.osei', 'mia.jensen', 'luca.bianchi', 'ana.silva', 'felix.wagner', 'yara.saleh', 'ben.carter', 'iris.tanaka'];
export const LABELS = ['bug', 'checkout', 'search', 'billing', 'auth', 'mobile', 'api', 'infra', 'docs', 'perf', 'ux', 'security', 'data', 'email', 'onboarding', 'reports', 'admin', 'i18n', 'testing', 'tech-debt', 'growth', 'support', 'payments', 'a11y'];
export const ZONES = ['Europe/Berlin', 'America/New_York', 'Asia/Tokyo', 'Australia/Sydney', 'America/Sao_Paulo', 'Asia/Kolkata', 'Africa/Cairo', 'Pacific/Auckland', 'America/Los_Angeles', 'Europe/London', 'Asia/Dubai', 'UTC'];
const VERBS = ['Fix', 'Add', 'Remove', 'Refactor', 'Investigate', 'Document', 'Migrate', 'Optimize', 'Review', 'Update', 'Test', 'Automate', 'Simplify', 'Rename', 'Split', 'Cache', 'Validate', 'Instrument', 'Backfill', 'Deprecate'];
const NOUNS = ['checkout', 'invoice', 'search index', 'login flow', 'session', 'webhook', 'export job', 'dashboard', 'notification', 'retry policy', 'rate limit', 'schema', 'onboarding', 'pricing page', 'cart', 'coupon', 'refund', 'audit log', 'permissions', 'importer', 'sitemap', 'avatar upload', 'password reset', 'report', 'tax rules', 'address form', 'inventory sync', 'email template', 'feature flag', 'cache layer'];
const DETAILS = ['for large accounts', 'on slow networks', 'after the last release', 'before the audit', 'in the mobile app', 'when the queue is full', 'behind a flag', 'for EU customers', 'without downtime', 'with better errors', 'in staging', 'for the new plan', 'on retry', 'in the admin console'];
export const TITLE_WORDS = [...new Set([...VERBS, ...NOUNS.flatMap((n) => n.split(' ')), ...DETAILS.flatMap((d) => d.split(' ').filter((w) => w.length > 3))].map((w) => w.toLowerCase()))].sort();
const STATUS_WEIGHTS: [string, number][] = [['todo', 0.34], ['doing', 0.18], ['blocked', 0.06], ['done', 0.42]];
const TRICKY = ['Handle "quoted" titles, commas and pipes | in exports', 'Support tab\tand ünïcödé names in labels', 'Escape *stars*, _underscores_ and [brackets] in Markdown'];

export interface WorkspaceSpec {
  tasks: number;
  /** Packages in the workspace-root node_modules (besides the two plugins). */
  rootPackages: number;
  webPackages: number;
  outerPackages: number;
  seed: number;
}

export const BENCH_SPEC: WorkspaceSpec = { tasks: 3000, rootPackages: 900, webPackages: 300, outerPackages: 150, seed: 7 };
export const GOLDEN_SPEC: WorkspaceSpec = { tasks: 240, rootPackages: 40, webPackages: 10, outerPackages: 10, seed: 7 };

export function generateTasks(count: number, seed: number): unknown[] {
  const rng = createRng(seed);
  const day = (offset: number) => new Date(Date.UTC(2026, 0, 1) + offset * 86_400_000).toISOString().slice(0, 10);
  const tasks: unknown[] = [];
  for (let id = 1; id <= count; id++) {
    const r = rng();
    let acc = 0;
    const status = STATUS_WEIGHTS.find(([, w]) => (acc += w) >= r)![0];
    const labels = [...new Set(Array.from({ length: Math.floor(rng() * 3) }, () => LABELS[Math.floor(rng() ** 1.5 * LABELS.length)]!))];
    const hasDue = rng() < 0.62;
    tasks.push({
      id,
      title: id % 97 === 0 ? TRICKY[(id / 97 - 1) % TRICKY.length]! : `${pick(rng, VERBS)} ${pick(rng, NOUNS)} ${pick(rng, DETAILS)}`,
      status,
      priority: 1 + Math.floor(rng() ** 1.4 * 4),
      assignee: ASSIGNEES[Math.floor(rng() ** 1.3 * ASSIGNEES.length)]!,
      labels,
      estimate: Math.round((0.5 + rng() ** 2 * 40) * 2) / 2,
      created: day(-200 + Math.floor(rng() * 230)),
      due: hasDue ? day(Math.floor(rng() * 160) - 20) : null,
    });
  }
  return tasks;
}

function writeJson(file: string, value: unknown): void {
  writeFileSync(file, JSON.stringify(value));
}

function fillNodeModules(dir: string, count: number, tag: string, plugins: { name: string; version: string }[] = []): void {
  const modules = join(dir, 'node_modules');
  mkdirSync(modules, { recursive: true });
  for (let i = 0; i < count; i++) {
    const name = `${['lodash', 'chalk', 'core', 'util', 'ms', 'debug', 'tslib', 'semver', 'glob', 'yargs'][i % 10]}-${tag}${i}`;
    mkdirSync(join(modules, name));
    writeJson(join(modules, name, 'package.json'), { name, version: `${1 + (i % 7)}.${i % 13}.${i % 5}`, main: 'index.js', license: 'MIT', keywords: ['util', tag] });
  }
  for (const p of plugins) {
    const target = join(modules, p.name);
    mkdirSync(target, { recursive: true });
    writeJson(join(target, 'package.json'), { name: p.name, version: p.version, keywords: ['taskr-plugin'] });
  }
}

export interface Workspace {
  /** Directory of the temp tree; removed by `cleanup()`. */
  base: string;
  /** The workspace root (where the applying `.taskrrc.json` lives). */
  root: string;
  /** The directory every invocation runs in. */
  cwd: string;
  /** Scratch dir used as HOME / TMPDIR / XDG_CACHE_HOME of the invocations; wiped between runs. */
  scratch: string;
  cleanup(): void;
}

export function buildWorkspace(partial: Partial<WorkspaceSpec> = {}, parent = tmpdir()): Workspace {
  const spec = { ...BENCH_SPEC, ...partial };
  const base = mkdtempSync(join(parent, 'taskr-ws-'));
  const outer = join(base, 'outer');
  const root = join(outer, 'ws');
  const cwd = join(root, 'packages', 'web', 'src', 'components', 'forms');
  mkdirSync(cwd, { recursive: true });
  mkdirSync(join(root, 'data'), { recursive: true });
  // a decoy above the root: the root config must stop the search before it
  writeJson(join(outer, '.taskrrc.json'), { tz: 'Asia/Tokyo', pageSize: 99, dataFile: 'nothing.json' });
  writeJson(join(outer, 'package.json'), { name: 'outer', private: true, taskr: { pageSize: 98 } });
  fillNodeModules(outer, spec.outerPackages, 'o');
  writeJson(join(root, '.taskrrc.json'), { root: true, dataFile: 'data/tasks.json', tz: 'Europe/Berlin', pageSize: 15 });
  writeJson(join(root, 'package.json'), { name: 'acme-monorepo', private: true, workspaces: ['packages/*'], devDependencies: Object.fromEntries(Array.from({ length: 60 }, (_, i) => [`dev-dep-${i}`, `^${1 + (i % 5)}.0.0`])) });
  writeJson(join(root, 'data', 'tasks.json'), generateTasks(spec.tasks, spec.seed));
  fillNodeModules(root, spec.rootPackages, 'r', [
    { name: 'taskr-plugin-gantt', version: '0.4.1' },
    { name: '@taskr/plugin-burndown', version: '1.2.0' },
  ]);
  writeJson(join(root, 'packages', 'web', 'package.json'), { name: 'web', private: true, taskr: { pageSize: 12 } });
  fillNodeModules(join(root, 'packages', 'web'), spec.webPackages, 'w');
  const scratch = join(base, 'scratch');
  mkdirSync(scratch);
  return { base, root, cwd, scratch, cleanup: () => rmSync(base, { recursive: true, force: true }) };
}

/** The environment the CLI is run in: fixed clock and terminal, and empty HOME / TMPDIR / cache directories. */
export function cliEnv(ws: Workspace): NodeJS.ProcessEnv {
  return {
    PATH: process.env.PATH ?? '',
    HOME: ws.scratch,
    TMPDIR: ws.scratch,
    XDG_CACHE_HOME: ws.scratch,
    TERM: 'xterm',
    LANG: 'C.UTF-8',
    TASKR_NOW: NOW,
  };
}

/** Empty the scratch directory: whatever a previous invocation cached must not help the next one. */
export function resetScratch(ws: Workspace): void {
  rmSync(ws.scratch, { recursive: true, force: true });
  mkdirSync(ws.scratch);
}
