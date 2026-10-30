import { mkdtempSync, readFileSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { LineSplitter, nodeSpawner, runProcess } from '../src/index.js';

const env = { PATH: process.env['PATH'] ?? '' };

describe('LineSplitter', () => {
  it('handles CRLF, blank lines and partial tails', () => {
    const s = new LineSplitter();
    expect(s.push('a\r\n\nb')).toEqual(['a']);
    expect(s.push('c\nd')).toEqual(['bc']);
    expect(s.flush()).toEqual(['d']);
    expect(s.flush()).toEqual([]);
  });
});

describe('runProcess with the real node spawner', () => {
  it('streams stdout lines, feeds stdin and captures the stderr tail', async () => {
    const lines: string[] = [];
    const res = await runProcess(nodeSpawner, 'sh', ['-c', 'read x; echo "got $x"; echo oops >&2; exit 3'], {
      cwd: tmpdir(), env, stdin: 'hello\n', onStdoutLine: (l) => lines.push(l),
    });
    expect(lines).toEqual(['got hello']);
    expect(res).toMatchObject({ code: 3, aborted: false, stderrTail: 'oops' });
  });

  it('reports a missing binary as a spawn error', async () => {
    const res = await runProcess(nodeSpawner, 'definitely-not-a-binary', [], { cwd: tmpdir(), env });
    expect(res.signal).toMatch(/^spawn-error/);
  });

  it('abort kills the whole process group, including grandchildren that ignore SIGTERM', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'pg-'));
    const pidFile = join(dir, 'grandchild.pid');
    // the shell starts a grandchild that ignores SIGTERM, records its pid, then both sleep
    const script = `(trap '' TERM; echo $$ > ${pidFile}; sleep 60) & sleep 60`;
    const ac = new AbortController();
    const p = runProcess(nodeSpawner, 'sh', ['-c', script], { cwd: dir, env, signal: ac.signal, killGraceMs: 200 });
    for (let i = 0; i < 100 && !existsSync(pidFile); i++) await new Promise((r) => setTimeout(r, 20));
    const grandchild = Number(readFileSync(pidFile, 'utf8').trim());
    expect(() => process.kill(grandchild, 0)).not.toThrow(); // alive
    ac.abort();
    const res = await p;
    expect(res.aborted).toBe(true);
    // SIGKILL after the grace period reaps the SIGTERM-ignoring grandchild too
    let alive = true;
    for (let i = 0; i < 50 && alive; i++) {
      try { process.kill(grandchild, 0); await new Promise((r) => setTimeout(r, 20)); } catch { alive = false; }
    }
    expect(alive).toBe(false);
  });
});
