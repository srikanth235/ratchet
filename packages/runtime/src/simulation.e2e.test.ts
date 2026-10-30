/**
 * Full simulated optimization on examples/demo-search: real git, real bench runs, real runner loop with
 * scripted agents. Takes a few minutes, so it only runs on request:
 *
 *   RATCHET_E2E=1 pnpm --filter @ratchet/runtime test:e2e
 */
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { afterAll, describe, expect, it } from 'vitest';
import { simulate } from './bin/simulate.js';

const enabled = process.env.RATCHET_E2E === '1';
const repoRoot = fileURLToPath(new URL('../../../', import.meta.url));

describe.skipIf(!enabled)('simulation e2e (RATCHET_E2E=1)', () => {
  let workRoot: string | undefined;
  afterAll(() => {
    if (workRoot && !process.env.RATCHET_E2E_KEEP) rmSync(workRoot, { recursive: true, force: true });
  });

  it(
    'runs two agents x 2 attempts to a verified win, drains every candidate, opens a PR and leaves nothing running',
    async () => {
      // Whether a scripted fix beats the noise floor depends on how busy the machine is (2 s load runs, 2
      // repeats). Everything else is checked on every run; only "no verified win at all" is retried.
      const MAX_RUNS = 3;
      const history: string[] = [];
      for (let attempt = 1; attempt <= MAX_RUNS; attempt++) {
        const lines: string[] = [];
        // A small noise multiplier: this test exercises the pipeline, not the statistics.
        const r = await simulate({ repoRoot, attempts: 2, durationSec: 2, repeats: 2, minutes: 12, settings: { noise: { k: 0.25 } }, log: (s) => lines.push(s) });
        if (workRoot) rmSync(workRoot, { recursive: true, force: true });
        workRoot = r.workRoot;
        const dump = () => `\n${lines.join('\n')}`;

        // ended cleanly (teardown moves `stopped` on to `completed`)
        expect(['stopped', 'completed'], dump()).toContain(r.summary.status);
        expect(['stopped', 'completed']).toContain(r.ledger.getOptimization(r.id)!.status);

        // every submitted candidate has a final verdict (the graceful stop drained the queue)
        const candidates = r.ledger.candidatesWithResults(r.id);
        expect(candidates.length).toBeGreaterThanOrEqual(2);
        expect(candidates.filter((c) => c.verdict === 'pending').map((c) => `${c.agentId}:${c.sha.slice(0, 8)}`), dump()).toEqual([]);
        // candidates are keyed per (agent, sha): no duplicate pair, even if two agents produced the same commit
        expect(new Set(candidates.map((c) => `${c.agentId}:${c.sha}`)).size).toBe(candidates.length);

        // the scripted protected-path attempt (claude-code attempt 2 edits bench/local.ts) is blocked, never benched
        const protectedAttempts = candidates.filter((c) => /protected path|bench load/i.test(`${c.message} ${c.reasons.join(' ')}`));
        expect(protectedAttempts.length, dump()).toBeGreaterThanOrEqual(1);
        for (const c of protectedAttempts) {
          expect(c.verdict).toBe('blocked');
          expect(c.results).toEqual([]);
        }

        // teardown: no tagged instance of this optimization is left alive in the mock cloud
        const live = (await r.compute.listTagged(r.id)).filter((i) => i.state !== 'terminated');
        expect(live).toEqual([]);
        expect(r.engine.teardownReport(r.id)).toMatchObject({ orphans: [], errors: [] });

        if (r.summary.verifiedWins < 1) {
          history.push(`run ${attempt}: no verified win: ${candidates.map((c) => `${c.verdict}(${c.reasons[0] ?? ''})`).join('; ')}`);
          continue;
        }

        // the ratchet only ever moves forward (minimize: strictly lower), each move starting where the last ended
        expect(r.history.length).toBeGreaterThanOrEqual(1);
        expect(r.history[0]!.fromValue).toBe(r.summary.baselineValue);
        for (const [i, m] of r.history.entries()) {
          expect(m.toValue, `move ${i}`).toBeLessThan(m.fromValue);
          if (i > 0) expect(m.fromValue).toBe(r.history[i - 1]!.toValue);
        }
        expect(r.summary.bestValue).toBe(r.history.at(-1)!.toValue);
        expect(candidates.filter((c) => c.verdict === 'verified')).toHaveLength(r.history.length);

        // PR written to disk
        expect(r.pr, dump()).toBeDefined();
        const prPath = fileURLToPath(r.pr!);
        expect(existsSync(prPath)).toBe(true);
        expect(readFileSync(prPath, 'utf8')).toMatch(/Ratchet/);
        return;
      }
      expect.fail(`no verified win in ${MAX_RUNS} runs (machine too noisy?)\n${history.join('\n')}`);
    },
    45 * 60_000,
  );
});
