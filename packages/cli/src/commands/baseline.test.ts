import { describe, expect, it } from 'vitest';
import { bufferIo } from '../io.js';
import { makeRepo } from '../testing.js';
import { runBaseline } from './baseline.js';

describe('runBaseline', () => {
  it('runs bench.repeats times and prints stats and the noise floor', async () => {
    const { config } = makeRepo({ values: [100, 102, 98] });
    const io = bufferIo();
    expect(await runBaseline(config, {}, io)).toBe(0);
    expect(io.stderr).toBe('');
    expect(io.stdout).toContain('run 1/3: lat.p95 = 100');
    expect(io.stdout).toContain('run 3/3: lat.p95 = 98');
    expect(io.stdout).toMatch(/mean\s+100\b/);
    expect(io.stdout).toMatch(/median\s+100\b/);
    expect(io.stdout).toMatch(/stddev\s+2\b/);
    expect(io.stdout).toMatch(/noise floor 4\b/);
    expect(io.stdout).toMatch(/required gain 4\b/); // max(4, 0.03 * 100 = 3)
  });

  it('--repeats overrides the config', async () => {
    const { config } = makeRepo({ values: [10, 20] });
    const io = bufferIo();
    expect(await runBaseline(config, { repeats: 4, json: true }, io)).toBe(0);
    const out = JSON.parse(io.stdout);
    expect(out.samples).toEqual([10, 20, 10, 20]);
    expect(out.repeats).toBe(4);
  });

  it('--json prints a single JSON document and nothing else on stdout', async () => {
    const { config } = makeRepo({ values: [100, 102, 98] });
    const io = bufferIo();
    expect(await runBaseline(config, { json: true }, io)).toBe(0);
    const out = JSON.parse(io.stdout);
    expect(out).toMatchObject({
      metric: 'lat.p95',
      direction: 'minimize',
      repeats: 3,
      samples: [100, 102, 98],
      noiseFloor: 4,
      noiseFloorFormula: { k: 2, minAbsolute: 0 },
      minGain: 0.03,
      requiredGain: 4,
    });
    expect(out.summary).toMatchObject({ n: 3, mean: 100, median: 100, stddev: 2, min: 98, max: 102 });
    expect(out.summary.cv).toBeCloseTo(0.02, 12);
  });

  it('honours --k and --min-absolute', async () => {
    const { config } = makeRepo({ values: [100, 102, 98] });
    let io = bufferIo();
    await runBaseline(config, { json: true, k: 3 }, io);
    expect(JSON.parse(io.stdout).noiseFloor).toBe(6);
    io = bufferIo();
    await runBaseline(config, { json: true, minAbsolute: 10 }, io);
    expect(JSON.parse(io.stdout).noiseFloor).toBe(10);
  });

  it('min_gain dominates required gain when runs are stable', async () => {
    const { config } = makeRepo({ values: [200, 200, 200] });
    const io = bufferIo();
    await runBaseline(config, { json: true }, io);
    const out = JSON.parse(io.stdout);
    expect(out.noiseFloor).toBe(0);
    expect(out.requiredGain).toBe(6); // 0.03 * 200
  });

  it('runs in the config file directory and uses the last JSON object, ignoring other output', async () => {
    // bench.mjs prints a non-JSON line with braces first; counter.txt must land next to the config
    const { config, dir } = makeRepo({ values: [5] });
    const io = bufferIo();
    expect(await runBaseline(config, { repeats: 1 }, io)).toBe(0);
    const { existsSync } = await import('node:fs');
    expect(existsSync(`${dir}/counter.txt`)).toBe(true);
    expect(io.stdout).toContain('warning: fewer than 3 runs');
  });

  it('fails when the metric is missing from the output', async () => {
    const { config } = makeRepo({ values: [1], metric: 'nope.p99' });
    const io = bufferIo();
    expect(await runBaseline(config, {}, io)).toBe(1);
    expect(io.stderr).toContain('run 1/3: metric "nope.p99" not found');
  });

  it('fails when the metric is not a number', async () => {
    const { config } = makeRepo({ values: [1], template: '{"lat":{"p95":"$V"}}' });
    const io = bufferIo();
    expect(await runBaseline(config, {}, io)).toBe(1);
    expect(io.stderr).toContain('not a finite number');
  });

  it('fails when stdout has no JSON', async () => {
    const { config } = makeRepo({ values: [1], benchRun: `node -e "console.log('hi')"` });
    const io = bufferIo();
    expect(await runBaseline(config, {}, io)).toBe(1);
    expect(io.stderr).toContain('no JSON object found on stdout');
  });

  it('fails with stderr tail when the bench exits non-zero', async () => {
    const { config } = makeRepo({
      values: [1],
      benchRun: `node -e "console.error('boom'); process.exit(3)"`,
    });
    const io = bufferIo();
    expect(await runBaseline(config, {}, io)).toBe(1);
    expect(io.stderr).toContain('exited with status 3');
    expect(io.stderr).toContain('boom');
  });

  it('times out a hung run', async () => {
    const { config } = makeRepo({ values: [1], benchRun: `node -e "setTimeout(()=>{}, 30000)"` });
    const io = bufferIo();
    expect(await runBaseline(config, { timeoutMs: 300 }, io)).toBe(1);
    expect(io.stderr).toContain('timed out after 300ms');
  });

  it('rejects an invalid config before running anything', async () => {
    const io = bufferIo();
    expect(await runBaseline('/nonexistent/ratchet.yaml', {}, io)).toBe(1);
    expect(io.stderr).toContain('cannot find');
  });

  it('rejects invalid repeats', async () => {
    const { config } = makeRepo({ values: [1] });
    const io = bufferIo();
    expect(await runBaseline(config, { repeats: 0 }, io)).toBe(1);
    expect(io.stderr).toContain('--repeats must be a positive integer');
  });
});
