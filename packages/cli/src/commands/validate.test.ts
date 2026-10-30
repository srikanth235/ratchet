import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { bufferIo } from '../io.js';
import { makeRepo } from '../testing.js';
import { runValidate } from './validate.js';

describe('runValidate', () => {
  it('prints a summary and exits 0 for a valid config', () => {
    const { config } = makeRepo({ values: [1] });
    const io = bufferIo();
    expect(runValidate(config, io)).toBe(0);
    expect(io.stderr).toBe('');
    expect(io.stdout).toContain('OK');
    expect(io.stdout).toContain('minimize lat.p95 (min gain 3%)');
    expect(io.stdout).toContain('node bench.mjs');
    expect(io.stdout).toContain('pytest -q; error_rate < 0.001');
    expect(io.stdout).toContain('bench/**, ratchet.yaml');
  });

  it('prints errors with YAML paths and exits 1 for an invalid config', () => {
    const dir = mkdtempSync(join(tmpdir(), 'ratchet-cli-'));
    const p = join(dir, 'ratchet.yaml');
    writeFileSync(
      p,
      `version: 0
objective: { metric: m, direction: sideways, min_gain: 0.03 }
bench: { ref: r, run: x, repeats: 3, hardware: h }
guardrails:
  - error_rate: "around 1"
agent: { instructions: R.md, allowed_instance_types: [a] }
`,
    );
    const io = bufferIo();
    expect(runValidate(p, io)).toBe(1);
    expect(io.stdout).toBe('');
    expect(io.stderr).toContain('invalid (2 errors)');
    expect(io.stderr).toContain('objective.direction: direction must be');
    expect(io.stderr).toContain('guardrails[0].error_rate: invalid threshold "around 1"');
  });

  it('exits 1 when the file is missing', () => {
    const io = bufferIo();
    expect(runValidate('/nonexistent/ratchet.yaml', io)).toBe(1);
    expect(io.stderr).toContain('cannot find /nonexistent/ratchet.yaml');
  });

  it('defaults to ./ratchet.yaml in the cwd', () => {
    const { dir } = makeRepo({ values: [1] });
    const prev = process.cwd();
    process.chdir(dir);
    try {
      const io = bufferIo();
      expect(runValidate(undefined, io)).toBe(0);
    } finally {
      process.chdir(prev);
    }
  });
});
