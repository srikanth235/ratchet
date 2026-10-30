import { describe, expect, it } from 'vitest';
import { bufferIo } from './io.js';
import { runCli } from './program.js';
import { makeRepo } from './testing.js';

describe('runCli', () => {
  it('validate [path]', async () => {
    const { config } = makeRepo({ values: [1] });
    const io = bufferIo();
    expect(await runCli(['validate', config], io)).toBe(0);
    expect(io.stdout).toContain('OK');
  });

  it('validate returns 1 for a missing file', async () => {
    const io = bufferIo();
    expect(await runCli(['validate', '/nonexistent/ratchet.yaml'], io)).toBe(1);
  });

  it('baseline [path] --repeats N --json', async () => {
    const { config } = makeRepo({ values: [1, 2, 3] });
    const io = bufferIo();
    expect(await runCli(['baseline', config, '--repeats', '3', '--json'], io)).toBe(0);
    expect(JSON.parse(io.stdout).samples).toEqual([1, 2, 3]);
  });

  it('baseline propagates failures as exit code 1', async () => {
    const { config } = makeRepo({ values: [1], metric: 'missing' });
    const io = bufferIo();
    expect(await runCli(['baseline', config], io)).toBe(1);
  });

  it('rejects bad option values with a non-zero exit code', async () => {
    const { config } = makeRepo({ values: [1] });
    const io = bufferIo();
    expect(await runCli(['baseline', config, '--repeats', 'abc'], io)).not.toBe(0);
    expect(io.stderr).toContain('--repeats');
  });

  it('unknown command is a usage error; --help exits 0', async () => {
    expect(await runCli(['frobnicate'], bufferIo())).not.toBe(0);
    const io = bufferIo();
    expect(await runCli(['--help'], io)).toBe(0);
    expect(io.stdout).toContain('validate');
    expect(io.stdout).toContain('baseline');
  });
});
