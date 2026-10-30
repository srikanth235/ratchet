import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import type { AgentEvent, RunnerHandle, RunnerMessage } from '@ratchet/core';
import {
  LocalRunnerControl,
  ScriptedAgent,
  createLocalAgentControl,
  detectHarnesses,
  localAdapterFactory,
  localAgentProblem,
  resolveLoginPath,
  type AgentScript,
  type HarnessInfo,
} from '../src/index.js';
import { autoVerdicts, baseConfig, makeSandbox, sh, until, type Sandbox } from './harness.js';

const cleanups: (() => void)[] = [];
afterEach(() => void cleanups.splice(0).forEach((c) => c()));
const tmp = (): string => {
  const d = mkdtempSync(join(tmpdir(), 'ratchet-local-'));
  cleanups.push(() => rmSync(d, { recursive: true, force: true }));
  return d;
};

const handle = (agentId: string, optimizationId = 'opt1'): RunnerHandle => ({ optimizationId, agentId, instanceId: `local:${agentId}` });

/** Plays the engine for one agent until its loop ends. */
async function drive(control: LocalRunnerControl, h: RunnerHandle, sb: Sandbox): Promise<AgentEvent[]> {
  const answer = autoVerdicts({ push: (m: RunnerMessage) => void control.send(h, m) });
  const seen: AgentEvent[] = [];
  let cursor: string | undefined;
  let finished = false;
  void control.result(h).then(() => (finished = true));
  const pump = async () => {
    const r = await control.poll(h, cursor);
    cursor = r.cursor;
    for (const e of r.events) {
      seen.push(e);
      answer(e);
    }
  };
  while (!finished) {
    await pump();
    await new Promise((r) => setTimeout(r, 10));
  }
  await pump();
  void sb;
  return seen;
}

const oneChange = (file: string): AgentScript => ({ name: 'w', attempts: [{ name: 'a', steps: [{ type: 'write', path: file, content: `${file}\n` }] }] });

describe('LocalRunnerControl in worktree mode', () => {
  const make = (sb: Sandbox, root: string, scripts: Record<string, AgentScript>) =>
    new LocalRunnerControl({
      rootDir: root,
      workdirs: 'worktree',
      adapterFor: (config) => new ScriptedAgent(scripts[config.agentId]!),
      loop: { maxIdleTurns: 1, retryDelayMs: 1 },
      stopGraceMs: 2000,
    });

  it('gives each agent a git worktree of one local clone, with its own identity, branch and refs', async () => {
    const sb = makeSandbox(undefined, ['ratchet/a', 'ratchet/b']);
    cleanups.push(sb.cleanup);
    const root = join(tmp(), 'agents');
    const control = make(sb, root, { a: oneChange('a.txt'), b: oneChange('b.txt') });
    const ha = handle('a');
    const hb = handle('b');
    await control.start(ha, baseConfig(sb, { agentId: 'a', branch: 'ratchet/a' }));
    await control.start(hb, baseConfig(sb, { agentId: 'b', branch: 'ratchet/b' }));

    const clone = join(root, 'opt1', 'clone');
    expect(control.workdirOf(ha)).toBe(join(root, 'opt1', 'a'));
    expect(existsSync(join(clone, '.git'))).toBe(true);
    // a worktree has a `.git` *file*, not a directory: it is not a second clone
    expect(readFileSync(join(control.workdirOf(ha), '.git'), 'utf8')).toMatch(/^gitdir: .*worktrees\/a/);
    expect(sh(clone, 'worktree', 'list').split('\n')).toHaveLength(3);

    const [seenA] = await Promise.all([drive(control, ha, sb), drive(control, hb, sb)]);
    expect(seenA.filter((e) => e.type === 'commit')).toHaveLength(1);

    // both branches reached the remote with their own file, authored by the agent's own identity, and no shared-ref clobbering
    expect(sh(sb.remote, 'show', 'ratchet/a:a.txt')).toBe('a.txt');
    expect(sh(sb.remote, 'show', 'ratchet/b:b.txt')).toBe('b.txt');
    // (the commit author itself can be overridden by GIT_AUTHOR_* in the caller's environment, so check the config git uses)
    expect(sh(control.workdirOf(ha), 'config', 'user.name')).toBe('a (ratchet)');
    expect(sh(control.workdirOf(hb), 'config', 'user.email')).toBe('b@ratchet.local');
    expect(sh(clone, 'for-each-ref', '--format=%(refname)', 'refs/ratchet')).toContain('refs/ratchet/a/accepted');
    expect(sh(clone, 'for-each-ref', '--format=%(refname)', 'refs/ratchet')).toContain('refs/ratchet/b/accepted');
    expect(sh(control.workdirOf(ha), 'config', '--worktree', 'commit.gpgsign')).toBe('false');
    await control.stopAll();
  });

  it('stop removes that agent\'s worktree and nothing else; release removes the clone', async () => {
    const sb = makeSandbox(undefined, ['ratchet/a', 'ratchet/b']);
    cleanups.push(sb.cleanup);
    const root = join(tmp(), 'agents');
    const slow: AgentScript = { name: 's', attempts: [{ name: 'a', steps: [{ type: 'sleep', ms: 60_000 }] }] };
    const control = make(sb, root, { a: slow, b: slow });
    const ha = handle('a');
    const hb = handle('b');
    await control.start(ha, baseConfig(sb, { agentId: 'a', branch: 'ratchet/a' }));
    await control.start(hb, baseConfig(sb, { agentId: 'b', branch: 'ratchet/b' }));
    await until(async () => (await control.poll(ha)).events.some((e) => e.text === 'agent turn started'));

    await control.stop(ha);
    expect(existsSync(join(root, 'opt1', 'a'))).toBe(false);
    expect(existsSync(join(root, 'opt1', 'b'))).toBe(true);
    expect(sh(join(root, 'opt1', 'clone'), 'worktree', 'list')).not.toContain('/a ');

    await control.release('opt1');
    expect(existsSync(join(root, 'opt1'))).toBe(false);
    // the remote is untouched
    expect(sh(sb.remote, 'rev-parse', 'ratchet/a')).toMatch(/^[0-9a-f]{40}$/);
    await control.release('opt1'); // idempotent
  });

  it('a start that cannot clone fails cleanly, is not registered, and can be retried', async () => {
    const sb = makeSandbox();
    cleanups.push(sb.cleanup);
    const control = make(sb, join(tmp(), 'agents'), { a: oneChange('a.txt') });
    const h = handle('a');
    await expect(control.start(h, baseConfig(sb, { repoUrl: join(sb.dir, 'missing.git') }))).rejects.toThrow(/clone/);
    await expect(control.send(h, { type: 'pause' })).rejects.toThrow(/unknown runner/);
    await control.start(h, baseConfig(sb));
    await control.stopAll();
  });

  it('a second run of the same optimization reuses (fetches into) the existing clone and replaces a stale worktree', async () => {
    const sb = makeSandbox();
    cleanups.push(sb.cleanup);
    const root = join(tmp(), 'agents');
    const first = make(sb, root, { a: oneChange('a.txt') });
    await first.start(handle('a'), baseConfig(sb));
    await drive(first, handle('a'), sb);
    // crash: no stop, no release. A new control (new process) starts the same agent again.
    const second = make(sb, root, { a: oneChange('a2.txt') });
    await second.start(handle('a'), baseConfig(sb));
    await drive(second, handle('a'), sb);
    expect(sh(sb.remote, 'show', 'ratchet/a:a2.txt')).toBe('a2.txt');
    await second.release('opt1');
  });

  it('poll reports an unknown (lost) agent as failed once, so the engine ends it', async () => {
    const sb = makeSandbox();
    cleanups.push(sb.cleanup);
    const control = make(sb, join(tmp(), 'agents'), {});
    const first = await control.poll(handle('ghost'));
    expect(first.events).toHaveLength(1);
    expect(first.events[0]).toMatchObject({ type: 'status', data: { state: 'failed', reason: 'lost' } });
    expect((await control.poll(handle('ghost'), first.cursor)).events).toEqual([]);
  });
});

// ---------------------------------------------------------------- the real adapters against shim CLIs

const STREAM = [
  '{"type":"system","subtype":"init","session_id":"sess-1","model":"shim"}',
  '{"type":"assistant","message":{"content":[{"type":"text","text":"made a change"}]}}',
  '{"type":"result","subtype":"success","is_error":false,"result":"done","num_turns":1,"duration_ms":5,"total_cost_usd":0.02,"usage":{"input_tokens":10,"output_tokens":5}}',
];

/** A `claude` stand-in: answers --version and `auth status`, and on a turn edits the worktree once and logs what it saw. */
function claudeShim(dir: string, seenFile: string): string {
  mkdirSync(dir, { recursive: true });
  const file = join(dir, 'claude');
  writeFileSync(
    file,
    [
      '#!/bin/sh',
      'case "$1" in',
      '  --version) echo "2.1.4 (Claude Code)"; exit 0;;',
      '  auth) echo \'{"loggedIn":true,"authMethod":"claude.ai"}\'; exit 0;;',
      'esac',
      'cat >/dev/null',
      `printf 'cwd=%s\\nkey=%s\\npath=%s\\ngh=%s\\naws=%s\\nargs=%s\\n' "$PWD" "$ANTHROPIC_API_KEY" "$PATH" "$GITHUB_TOKEN" "$AWS_SECRET_ACCESS_KEY" "$*" >> ${JSON.stringify(seenFile)}`,
      'case "$*" in *--resume*) ;; *) mkdir -p src; echo "shim edit" >> src/app.txt;; esac',
      ...STREAM.map((l) => `echo '${l}'`),
      '',
    ].join('\n'),
  );
  chmodSync(file, 0o755);
  return file;
}

describe('local agents with the real adapters (shim CLIs)', () => {
  const info = (path: string, auth: HarnessInfo['auth'] = 'signed-in'): HarnessInfo => ({ kind: 'claude-code', installed: true, version: '2.1.4', path, auth, hint: '' });

  it('runs ClaudeCodeAdapter with the detected binary, the user login PATH and no API key', async () => {
    const root = tmp();
    const seen = join(root, 'seen.txt');
    const bin = claudeShim(join(root, 'bin'), seen);
    const sb = makeSandbox();
    cleanups.push(sb.cleanup);
    const control = createLocalAgentControl({
      rootDir: join(root, 'agents'),
      harnesses: [info(bin)],
      loginPath: `${join(root, 'bin')}:/usr/bin:/bin`,
      baseEnv: { HOME: root, PATH: '/stripped', ANTHROPIC_API_KEY: 'sk-ant-MUSTNOTPASS', GITHUB_TOKEN: 'ghp_MUSTNOTPASS', AWS_SECRET_ACCESS_KEY: 'aws-MUSTNOTPASS' },
      loop: { maxIdleTurns: 1, retryDelayMs: 1 },
    });
    const h = handle('cc');
    await control.start(h, baseConfig(sb, { agentId: 'cc', kind: 'claude-code', branch: 'ratchet/a' }));
    const events = await drive(control, h, sb);

    expect(events.some((e) => e.type === 'commit')).toBe(true);
    expect(events.find((e) => e.type === 'usage')?.usage?.costUsd).toBe(0.02);
    expect(sh(sb.remote, 'show', 'ratchet/a:src/app.txt')).toContain('shim edit');

    const log = readFileSync(seen, 'utf8');
    expect(log).toContain(`cwd=${join(root, 'agents', 'opt1', 'cc')}`);
    expect(log).toMatch(/\nkey=\n/); // signed-in: the key in the environment is not passed
    expect(log).toMatch(/\ngh=\n/);
    expect(log).toMatch(/\naws=\n/);
    expect(log).toContain(`path=${join(root, 'bin')}:/usr/bin:/bin`);
    expect(log).not.toContain('MUSTNOTPASS');
    expect(log).toContain('--output-format stream-json');
    expect(sh(sb.remote, 'log', '-1', '--format=%B', 'ratchet/a')).toContain('Ratchet-Candidate: true');
    await control.release('opt1');
    expect(existsSync(join(root, 'agents', 'opt1'))).toBe(false);
  });

  it('passes the key only when the harness reports api-key auth', async () => {
    const root = tmp();
    const seen = join(root, 'seen.txt');
    const bin = claudeShim(join(root, 'bin'), seen);
    const sb = makeSandbox();
    cleanups.push(sb.cleanup);
    const control = createLocalAgentControl({
      rootDir: join(root, 'agents'),
      harnesses: [info(bin, 'api-key')],
      loginPath: '/usr/bin:/bin',
      baseEnv: { HOME: root, ANTHROPIC_API_KEY: 'sk-ant-OKTOPASS' },
      loop: { maxIdleTurns: 1, retryDelayMs: 1 },
    });
    const h = handle('cc');
    await control.start(h, baseConfig(sb, { agentId: 'cc', kind: 'claude-code', branch: 'ratchet/a' }));
    await drive(control, h, sb);
    expect(readFileSync(seen, 'utf8')).toContain('key=sk-ant-OKTOPASS');
    await control.release('opt1');
  });

  it('refuses to launch a harness that is missing or signed out, with the detection hint', async () => {
    const out: HarnessInfo = { kind: 'codex', installed: true, version: '1', path: '/x/codex', auth: 'signed-out', hint: 'Run `codex login`.' };
    const missing: HarnessInfo = { kind: 'claude-code', installed: false, version: null, path: null, auth: 'signed-out', hint: 'Install Claude Code.' };
    expect(localAgentProblem([out, missing], 'codex')).toBe('Run `codex login`.');
    expect(localAgentProblem([out, missing], 'claude-code')).toBe('Install Claude Code.');
    expect(localAgentProblem([], 'codex')).toMatch(/not probed/);
    expect(localAgentProblem([{ ...out, auth: 'signed-in' }], 'codex')).toBeNull();
    const factory = localAdapterFactory({ harnesses: [out, missing], loginPath: '/bin' });
    expect(() => factory(baseConfig({ remote: 'x' } as Sandbox, { kind: 'codex' }))).toThrow(/cannot run codex.*codex login/);
    expect(() => factory(baseConfig({ remote: 'x' } as Sandbox, { kind: 'mock' }))).toThrow(/mock agents/);
  });
});

describe('detection against real processes (shim CLIs)', () => {
  it('finds shims through the login-shell PATH, reads version and auth', async () => {
    const root = tmp();
    const bin = join(root, 'bin');
    claudeShim(bin, join(root, 'seen.txt'));
    writeFileSync(join(bin, 'codex'), '#!/bin/sh\ncase "$1" in --version) echo "codex-cli 0.46.0";; login) echo "Logged in using ChatGPT" >&2;; esac\n');
    chmodSync(join(bin, 'codex'), 0o755);
    const env = { PATH: `${bin}:/usr/bin:/bin`, HOME: root, SHELL: '/bin/sh' };
    const login = await resolveLoginPath({ env, home: root });
    expect(login.path.split(':')).toContain(bin);
    const [cc, cx] = await detectHarnesses({ env, home: root, loginPath: login.path });
    expect(cc).toMatchObject({ kind: 'claude-code', installed: true, version: '2.1.4', path: join(bin, 'claude'), auth: 'signed-in' });
    expect(cx).toMatchObject({ kind: 'codex', installed: true, version: '0.46.0', path: join(bin, 'codex'), auth: 'signed-in' });
  });

  it('a missing CLI is not installed and a hanging one times out', async () => {
    const root = tmp();
    const bin = join(root, 'bin');
    mkdirSync(bin);
    writeFileSync(join(bin, 'codex'), '#!/bin/sh\nsleep 30\n');
    chmodSync(join(bin, 'codex'), 0o755);
    const t0 = Date.now();
    const [cc, cx] = await detectHarnesses({ env: { PATH: `${bin}:/usr/bin:/bin`, HOME: root, SHELL: '/bin/sh' }, home: root, timeoutMs: 300 });
    expect(Date.now() - t0).toBeLessThan(5000);
    expect(cc).toMatchObject({ installed: false, path: null });
    expect(cx).toMatchObject({ installed: false, path: join(bin, 'codex') });
    expect(cx!.hint).toMatch(/timed out/);
  });
});
