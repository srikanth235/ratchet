import { execFile, spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { GOLDEN_CASES } from '../../bench/golden-cases.js';
import { GOLDEN_SPEC, buildWorkspace, cliEnv, type Workspace } from '../../bench/workspace.js';
import { PACKAGE_ROOT } from '../../src/paths.js';

/**
 * The CLI contract: the built `dist/src/main.js`, run in the generated workspace, must print exactly what
 * tests/golden/*.out records (and exit with the recorded code), byte for byte. Needs a build (`pnpm build`; `pnpm test` does it).
 */
const CLI = join(PACKAGE_ROOT, 'dist', 'src', 'main.js');
const GOLDEN_DIR = join(PACKAGE_ROOT, 'tests', 'golden');

interface Result {
  status: number | null;
  stdout: string;
  stderr: string;
}

let ws: Workspace;
const results = new Map<string, Result>();

const runAsync = (args: string[]) =>
  new Promise<Result>((resolve) => {
    execFile(process.execPath, [CLI, ...args], { cwd: ws.cwd, env: cliEnv(ws), encoding: 'utf8', maxBuffer: 64 << 20 }, (err, stdout, stderr) => {
      resolve({ status: err ? ((err as { code?: number }).code ?? null) : 0, stdout, stderr });
    });
  });

beforeAll(async () => {
  if (!existsSync(CLI)) throw new Error(`${CLI} does not exist: run \`pnpm build\` first`);
  ws = buildWorkspace(GOLDEN_SPEC);
  // the golden cases are independent processes: run them a few at a time
  const queue = [...GOLDEN_CASES];
  await Promise.all(
    Array.from({ length: 6 }, async () => {
      for (let c = queue.shift(); c; c = queue.shift()) results.set(c.name, await runAsync(c.args));
    }),
  );
});
afterAll(() => ws?.cleanup());

const run = (args: string[]) => spawnSync(process.execPath, [CLI, ...args], { cwd: ws.cwd, env: cliEnv(ws), encoding: 'utf8' });

describe('golden output', () => {
  it.each(GOLDEN_CASES.map((c) => [c.name, c] as const))('%s', (_name, c) => {
    const r = results.get(c.name)!;
    expect(r.status).toBe(c.exit);
    expect(r[c.stream]).toBe(readFileSync(join(GOLDEN_DIR, `${c.name}.out`), 'utf8'));
    expect(r[c.stream === 'stdout' ? 'stderr' : 'stdout']).toBe('');
  });

  it('every golden file belongs to a case, and every case has its file', () => {
    const files = readdirSync(GOLDEN_DIR).filter((f) => f.endsWith('.out')).map((f) => f.slice(0, -4));
    expect(files.sort()).toEqual(GOLDEN_CASES.map((c) => c.name).sort());
  });
});

describe('behaviour that must survive any startup optimization', () => {
  it('does not read configuration above a "root": true file', () => {
    const r = run(['config', '--format', 'json']);
    const config = JSON.parse(r.stdout) as { tz: string; pageSize: number; dataFile: string };
    expect(config.tz).toBe('Europe/Berlin'); // not the decoy's Asia/Tokyo
    expect(config.dataFile).toBe('data/tasks.json'); // not the decoy's nothing.json
    expect(config.pageSize).toBe(12); // the nearer package.json wins over the root file
  });

  it('prints help for no arguments, --help and -h; the version for --version and -v', () => {
    const help = run([]).stdout;
    expect(help).toContain('Usage: taskr <command>');
    expect(run(['--help']).stdout).toBe(help);
    expect(run(['-h']).stdout).toBe(help);
    expect(run(['--version']).stdout).toBe('taskr 1.4.2\n');
    expect(run(['-v']).stdout).toBe('taskr 1.4.2\n');
  });

  it('works outside any workspace: no config, no data file', () => {
    const r = spawnSync(process.execPath, [CLI, 'list'], { cwd: ws.scratch, env: cliEnv(ws), encoding: 'utf8' });
    expect(r.status).toBe(0);
    expect(r.stdout).toBe('No tasks match.\n');
  });

  it('wraps the title column to the COLUMNS of a narrow terminal', () => {
    const r = spawnSync(process.execPath, [CLI, 'list', '--limit', '3'], { cwd: ws.cwd, env: { ...cliEnv(ws), COLUMNS: '60' }, encoding: 'utf8' });
    for (const line of r.stdout.split('\n')) expect(line.length).toBeLessThanOrEqual(60);
  });
});
