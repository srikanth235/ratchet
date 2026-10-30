import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { DEMO_CLI_SCRIPT_DIR, loadDemoCliScript, type AgentScript } from '../src/index.js';
import { REPO_ROOT } from './demo-engine.js';

const DEMO = join(REPO_ROOT, 'examples/demo-cli');
const patchFiles = readdirSync(DEMO_CLI_SCRIPT_DIR).filter((f) => f.endsWith('.patch')).sort();
const patchOrder = (s: AgentScript) => s.attempts.flatMap((a) => a.steps.flatMap((st) => (st.type === 'patch' ? [st.name!] : [])));
const dirs: string[] = [];
afterEach(() => void dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

/** A throwaway copy of examples/demo-cli (no node_modules / dist), with the repo's node_modules linked in. */
function scratchTree(): string {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-cli-'));
  dirs.push(dir);
  cpSync(DEMO, dir, { recursive: true, filter: (src) => !/[\\/](node_modules|dist)([\\/]|$)/.test(src) });
  symlinkSync(join(REPO_ROOT, 'node_modules'), join(dir, 'node_modules'), 'dir');
  execFileSync('git', ['init', '-q'], { cwd: dir });
  return dir;
}
const apply = (dir: string, f: string, check = false) =>
  execFileSync('git', ['apply', ...(check ? ['--check'] : []), '--whitespace=nowarn', join(DEMO_CLI_SCRIPT_DIR, f)], { cwd: dir, stdio: 'pipe' });
/** Builds the demo and runs its unit + contract tests in `dir`; returns the failure output (empty when green). */
const failures = (dir: string): string => {
  try {
    execFileSync(process.execPath, [join(REPO_ROOT, 'node_modules/typescript/bin/tsc'), '-p', 'tsconfig.build.json'], { cwd: dir, stdio: 'pipe', timeout: 120_000 });
    execFileSync(process.execPath, [join(REPO_ROOT, 'node_modules/vitest/vitest.mjs'), 'run'], { cwd: dir, stdio: 'pipe', timeout: 240_000 });
    return '';
  } catch (err) {
    const e = err as { stdout?: Buffer; stderr?: Buffer };
    return `${e.stdout ?? ''}${e.stderr ?? ''}`.slice(-3000) || 'build or tests failed';
  }
};

describe('demo-cli patches', () => {
  it('ships the seven real improvements', () => {
    expect(patchFiles).toEqual([
      '01-lazy-message-catalogs.patch',
      '02-lazy-spelling-dictionary.patch',
      '03-lazy-time-zones.patch',
      '04-bounded-config-and-plugin-scan.patch',
      '05-no-tput-shell-out.patch',
      '06-lazy-context.patch',
      '07-lazy-commands.patch',
    ]);
  });

  // the cc and cx scripts play subsets of the seven, in different orders; every prefix applies and keeps the tests green
  const orders: Record<string, string[]> = {
    cc: ['03', '01', '02', '04', '05'],
    cx: ['01', '07', '02', '03', '06'],
  };
  it.each(Object.entries(orders))('applies cumulatively in the %s order and keeps the demo tests green', (_name, order) => {
    const dir = scratchTree();
    for (const n of order) {
      const f = patchFiles.find((p) => p.startsWith(`${n}-`))!;
      apply(dir, f, true);
      apply(dir, f);
    }
    expect(failures(dir)).toBe('');
  }, 300_000);
});

describe('demo-cli scripts', () => {
  it('cc and cx play real fixes in different orders and include every kind of bad attempt', () => {
    const cc = loadDemoCliScript('cc');
    const cx = loadDemoCliScript('cx');
    for (const s of [cc, cx]) {
      expect(new Set(patchOrder(s)).size).toBeGreaterThanOrEqual(5);
      const steps = s.attempts.flatMap((a) => a.steps);
      const edits = steps.filter((st) => st.type === 'edit') as { path: string; find: string }[];
      const writes = steps.filter((st) => st.type === 'write') as { path: string; content: string }[];
      expect(edits.some((e) => /^(bench\/|tests\/golden\/)/.test(e.path))).toBe(true); // protected path
      expect(writes.some((w) => w.path === 'src/snapshot.ts')).toBe(true); // overfit to the visible invocations
      expect(edits.some((e) => /^src\/(render|store)\b/.test(e.path))).toBe(true); // noise-level no-op
      expect(s.attempts.at(-1)!.name).toBe('wrap-up');
      for (const a of s.attempts) {
        const u = a.steps.find((st) => st.type === 'usage') as { inputTokens: number; outputTokens: number } | undefined;
        expect(u && u.inputTokens > 10_000 && u.outputTokens > 100).toBe(true);
      }
    }
    expect(patchOrder(cc)).not.toEqual(patchOrder(cx));
  });

  it('the overfit table holds exactly the outputs of the visible invocations that succeed', () => {
    const mix = JSON.parse(readFileSync(join(DEMO, 'bench/invocations.json'), 'utf8')) as { invocations: { args: string[]; expect?: number }[] };
    const expected = mix.invocations.filter((i) => (i.expect ?? 0) === 0).map((i) => i.args.join('\0'));
    const write = loadDemoCliScript('cc').attempts.flatMap((a) => a.steps).find((st) => st.type === 'write' && st.path === 'src/snapshot.ts') as { content: string };
    const table = JSON.parse(write.content.slice(write.content.indexOf('= {') + 2, write.content.lastIndexOf('};') + 1)) as Record<string, string>;
    expect(Object.keys(table).sort()).toEqual([...expected].sort());
  });
});
