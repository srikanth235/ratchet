import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, readdirSync, rmSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { DEMO_FORECAST_SCRIPT_DIR, loadDemoForecastScript, type AgentScript } from '../src/index.js';
import { REPO_ROOT } from './demo-engine.js';

const DEMO = join(REPO_ROOT, 'examples/demo-forecast');
const patchFiles = readdirSync(DEMO_FORECAST_SCRIPT_DIR).filter((f) => f.endsWith('.patch')).sort();
const patchOrder = (s: AgentScript) => s.attempts.flatMap((a) => a.steps.flatMap((st) => (st.type === 'patch' ? [st.name!] : [])));
const dirs: string[] = [];
afterEach(() => void dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

/** A throwaway git tree of examples/demo-forecast. */
function scratchTree(): string {
  const dir = mkdtempSync(join(tmpdir(), 'ratchet-forecast-'));
  dirs.push(dir);
  cpSync(DEMO, dir, { recursive: true, filter: (src) => !/[\\/](node_modules|dist)([\\/]|$)/.test(src) });
  symlinkSync(join(REPO_ROOT, 'node_modules'), join(dir, 'node_modules'), 'dir');
  return dir;
}
const apply = (dir: string, f: string, check = false) =>
  execFileSync('git', ['apply', ...(check ? ['--check'] : []), '--whitespace=nowarn', join(DEMO_FORECAST_SCRIPT_DIR, f)], { cwd: dir, stdio: 'pipe' });
/** Runs the demo's own tests in `dir`; returns the failure output (empty when green). */
const testFailures = (dir: string): string => {
  try {
    execFileSync(process.execPath, [join(REPO_ROOT, 'node_modules/vitest/vitest.mjs'), 'run'], { cwd: dir, stdio: 'pipe', timeout: 120_000 });
    return '';
  } catch (err) {
    const e = err as { stdout?: Buffer; stderr?: Buffer };
    return `${e.stdout ?? ''}${e.stderr ?? ''}`.slice(-3000) || 'vitest failed';
  }
};

describe('demo-forecast patches', () => {
  it('ships the five real improvements', () => {
    expect(patchFiles).toEqual([
      '01-seasonal-naive.patch',
      '02-smoothed-level-seasonal-index.patch',
      '03-promo-holiday-effects.patch',
      '04-pooled-seasonal-profile.patch',
      '05-stockout-bias-correction.patch',
    ]);
  });

  // 03 and 05 are independent of the model; 02 builds on 01; 04 builds on 02
  const orders: Record<string, string[]> = {
    cc: ['01', '02', '03', '04', '05'],
    cx: ['03', '01', '05', '02', '04'],
  };
  it.each(Object.entries(orders))('applies cumulatively in the %s order and keeps the demo tests green', (_name, order) => {
    const dir = scratchTree();
    for (const n of order) {
      const f = patchFiles.find((p) => p.startsWith(`${n}-`))!;
      apply(dir, f, true);
      apply(dir, f);
    }
    expect(testFailures(dir)).toBe('');
  }, 120_000);

  it('04 needs 02', () => {
    expect(() => apply(scratchTree(), '04-pooled-seasonal-profile.patch', true)).toThrow();
  });
});

describe('demo-forecast scripts', () => {
  it('cc and cx play the same fixes in different orders and include every kind of bad attempt', () => {
    const cc = loadDemoForecastScript('cc');
    const cx = loadDemoForecastScript('cx');
    for (const s of [cc, cx]) {
      expect(new Set(patchOrder(s)).size).toBeGreaterThanOrEqual(5);
      const steps = s.attempts.flatMap((a) => a.steps);
      const edits = steps.filter((st) => st.type === 'edit') as { path: string; find: string }[];
      expect(edits.some((e) => /^(eval|data)\//.test(e.path))).toBe(true); // protected path
      expect(edits.some((e) => e.find.includes('table.units') || e.find.includes('export function predict'))).toBe(true); // leakage / overfit
      expect(steps.some((st) => st.type === 'write' && st.path === 'src/calibration.ts')).toBe(true); // overfit
      expect(edits.some((e) => e.find === 'export const WINDOW = 4;')).toBe(true); // noise-level no-op
      expect(s.attempts.at(-1)!.name).toBe('wrap-up');
      for (const a of s.attempts) {
        const u = a.steps.find((st) => st.type === 'usage') as { inputTokens: number; outputTokens: number } | undefined;
        expect(u && u.inputTokens > 10_000 && u.outputTokens > 100).toBe(true);
      }
    }
    expect(patchOrder(cc)).not.toEqual(patchOrder(cx));
  });
});
