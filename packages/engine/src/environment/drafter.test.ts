import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
  formatAgentAnswer,
  type AgentAdapter,
  type AgentEvent,
  type AgentSession,
  type AgentSpec,
  type EnvironmentChecker,
  type EnvironmentDraftRecord,
  type PriceBook,
} from '@ratchet/core';
import { FakeClock, FakeGit } from '../testing/fakes.js';
import { AgentEnvironmentDrafter } from './drafter.js';
import { MOCK_CONFIG_YAML, sampleEnvironment } from './samples.js';

/** A setup agent that follows a script: each turn writes files into the workdir and answers like the brief asks. */
class ScriptedAgent implements AgentAdapter {
  readonly kind = 'claude-code' as const;
  readonly prompts: string[] = [];
  readonly resumed: AgentSession[] = [];
  preflightProblems: string[] = [];
  constructor(private readonly turns: Array<(dir: string) => Promise<string> | string>, private readonly usd = 0.5) {}
  private turn = 0;

  async preflight() {
    return { ok: this.preflightProblems.length === 0, problems: this.preflightProblems };
  }
  private async run(spec: AgentSpec, onEvent: (e: AgentEvent) => void): Promise<void> {
    this.prompts.push(spec.prompt);
    const text = await this.turns[Math.min(this.turn++, this.turns.length - 1)]!(spec.workdir);
    onEvent({ at: 0, type: 'tool', text: 'writing infra/ratchet/main.tf' });
    onEvent({ at: 0, type: 'message', text });
    onEvent({ at: 0, type: 'usage', usage: { inputTokens: 1000, outputTokens: 1000, costUsd: this.usd } });
  }
  async start(spec: AgentSpec, onEvent: (e: AgentEvent) => void): Promise<AgentSession> {
    await this.run(spec, onEvent);
    return { id: 'sess-1', kind: 'claude-code' };
  }
  async resume(session: AgentSession, spec: AgentSpec, _m: string, onEvent: (e: AgentEvent) => void): Promise<void> {
    this.resumed.push(session);
    await this.run(spec, onEvent);
  }
}

async function put(dir: string, files: Array<{ path: string; content: string }>): Promise<void> {
  for (const f of files) {
    await mkdir(dirname(join(dir, f.path)), { recursive: true });
    await writeFile(join(dir, f.path), f.content);
  }
}

/** The agent writes the sample environment of `scope` and answers. */
const writes = (scope: 'none' | 'single' | 'full', reason = `reason for ${scope}`, extra?: (dir: string) => Promise<void>) => async (dir: string): Promise<string> => {
  await put(dir, sampleEnvironment(scope, { originalConfigYaml: MOCK_CONFIG_YAML }).files);
  await extra?.(dir);
  return `Done.\n${formatAgentAnswer({ scope, reason })}`;
};

const input = { repo: 'https://example.test/acme/repo.git', goal: 'cut checkout p95', agentConnector: 'claude-code' as const };

function setup(agent: AgentAdapter, over: { checker?: EnvironmentChecker; prices?: PriceBook; configYaml?: string } = {}) {
  const clock = new FakeClock();
  const git = new FakeGit([], clock, { 'ratchet.yaml': over.configYaml ?? MOCK_CONFIG_YAML, 'src/app.ts': 'export {};\n' });
  let n = 0;
  const errors: string[] = [];
  const drafter = new AgentEnvironmentDrafter({
    git,
    adapterFor: () => agent,
    clock,
    newId: () => `env-${String(++n).padStart(4, '0')}`,
    ...(over.checker ? { checker: over.checker } : {}),
    ...(over.prices ? { prices: over.prices } : {}),
    onError: (e) => errors.push((e as Error).message),
  });
  return { drafter, git, clock, errors };
}

const passingChecker = (): EnvironmentChecker & { calls: number } => ({
  calls: 0,
  async plan() {
    this.calls++;
    return { ok: true, detail: 'tofu plan: 14 to add' };
  },
});

describe('AgentEnvironmentDrafter.draft', () => {
  it('runs the setup agent on a checkout, validates what it wrote, plans it in the account, prices it', async () => {
    const agent = new ScriptedAgent([writes('full')]);
    const checker = passingChecker();
    const { drafter } = setup(agent, { checker });
    const phases: string[] = [];
    const rec = await drafter.draft({ ...input, timeLimitSeconds: 3600, onProgress: (p) => phases.push(p.phase) });
    expect(rec).toMatchObject({ status: 'ready', revision: 1, baseRef: 'main' });
    const d = rec.draft!;
    expect(d).toMatchObject({ scope: 'full', reason: 'reason for full', agent: 'claude-code', usdSpent: 0.5 });
    expect(d.files[0]!.path).toBe('ratchet.yaml');
    expect(d.files.map((f) => f.path)).toContain('infra/ratchet/main.tf');
    expect(d.resources.length).toBe(4);
    expect(d.checks.every((c) => c.ok)).toBe(true);
    expect(d.checks.find((c) => c.id === 'plan')).toMatchObject({ ok: true, detail: 'tofu plan: 14 to add' });
    expect(checker.calls).toBe(1);
    expect(phases).toEqual(['reading', 'drafting', 'drafting', 'validating', 'done']);
    // the brief carries the goal and the contract
    expect(agent.prompts[0]).toContain('cut checkout p95');
    expect(agent.prompts[0]).toMatch(/infra\/ratchet/);
  });

  it('scope none / single with no OpenTofu need no plan: the standard sandbox is used', async () => {
    for (const scope of ['none', 'single'] as const) {
      const checker = passingChecker();
      const { drafter } = setup(new ScriptedAgent([writes(scope)]), { checker });
      const rec = await drafter.draft(input);
      expect(rec.draft!.scope).toBe(scope);
      expect(rec.draft!.checks.find((c) => c.id === 'plan')!.detail).toMatch(/no OpenTofu/);
      expect(checker.calls).toBe(0);
      expect(rec.draft!.checks.every((c) => c.ok)).toBe(true);
    }
  });

  it('does not claim a plan it did not run: no checker -> the plan check fails; a failing validator skips the plan', async () => {
    const { drafter } = setup(new ScriptedAgent([writes('full')]));
    const rec = await drafter.draft(input);
    const plan = rec.draft!.checks.find((c) => c.id === 'plan')!;
    expect(plan.ok).toBe(false);
    expect(plan.detail).toMatch(/connect AWS/);

    const checker = passingChecker();
    const bad = writes('full', 'x', async (dir) => {
      const p = join(dir, 'infra/ratchet/main.tf');
      await writeFile(p, (await readFile(p, 'utf8')) + '\nresource "aws_security_group_rule" "out" {\n  cidr_blocks = ["0.0.0.0/0"]\n  type = "egress"\n}\n');
    });
    const r2 = await setup(new ScriptedAgent([bad]), { checker }).drafter.draft(input);
    expect(r2.draft!.checks.some((c) => !c.ok && c.id !== 'plan')).toBe(true);
    expect(r2.draft!.checks.find((c) => c.id === 'plan')).toMatchObject({ ok: false, detail: expect.stringMatching(/fix the failed checks first/) });
    expect(checker.calls).toBe(0);
  });

  it('a checker that throws or rejects the plan fails the plan check (not the draft call)', async () => {
    const throws: EnvironmentChecker = { plan: async () => { throw new Error('no credentials'); } };
    const rec = await setup(new ScriptedAgent([writes('full')]), { checker: throws }).drafter.draft(input);
    expect(rec.status).toBe('ready');
    expect(rec.draft!.checks.find((c) => c.id === 'plan')).toMatchObject({ ok: false, detail: 'could not run tofu: no credentials' });
    const rejects: EnvironmentChecker = { plan: async () => ({ ok: false, detail: 'Error: invalid subnet' }) };
    const r2 = await setup(new ScriptedAgent([writes('full')]), { checker: rejects }).drafter.draft(input);
    expect(r2.draft!.checks.find((c) => c.id === 'plan')).toMatchObject({ ok: false, detail: 'Error: invalid subnet' });
  });

  it('a draft whose ratchet.yaml changed anything outside environment: fails the config check', async () => {
    const agent = new ScriptedAgent([
      async (dir) => {
        await put(dir, sampleEnvironment('none', { originalConfigYaml: MOCK_CONFIG_YAML.replace('minimize', 'maximize') }).files);
        return formatAgentAnswer({ scope: 'none', reason: 'r' });
      },
    ]);
    const rec = await setup(agent).drafter.draft(input);
    expect(rec.draft!.checks.find((c) => c.id === 'config')).toMatchObject({ ok: false, detail: expect.stringMatching(/objective/) });
  });

  it('writes the decision itself when the agent forgot environment.scope in ratchet.yaml', async () => {
    const agent = new ScriptedAgent([() => formatAgentAnswer({ scope: 'none', reason: 'tests only' })]);
    const rec = await setup(agent).drafter.draft(input);
    expect(rec.draft!.files[0]!.content).toContain('scope: none');
    expect(rec.draft!.checks.every((c) => c.ok)).toBe(true);
  });

  it('changes outside ratchet.yaml and infra/ratchet/ are reported and never part of the draft', async () => {
    const agent = new ScriptedAgent([writes('single', 'r', async (dir) => put(dir, [{ path: 'src/app.ts', content: 'export const x = 1;\n' }]))]);
    const rec = await setup(agent).drafter.draft(input);
    expect(rec.draft!.files.map((f) => f.path)).not.toContain('src/app.ts');
    expect(rec.messages.at(-1)!.text).toContain('also changed src/app.ts');
  });

  it('prefers live prices from the price book over the static table', async () => {
    const prices: PriceBook = { hourlyUsd: async (t) => (t === 'c7i.2xlarge' ? 10 : 0.01) } as PriceBook;
    const live = await setup(new ScriptedAgent([writes('full')]), { prices, checker: passingChecker() }).drafter.draft(input);
    const fixed = await setup(new ScriptedAgent([writes('full')]), { checker: passingChecker() }).drafter.draft(input);
    expect(live.draft!.cost.usdPerHour).toBeGreaterThan(10);
    expect(fixed.draft!.cost.usdPerHour).toBeLessThan(5);
  });

  it('fails cleanly (status failed, scratch released, error recorded) when the agent cannot run, there is no config, or no scope can be told', async () => {
    const unavailable = new ScriptedAgent([writes('none')]);
    unavailable.preflightProblems = ['claude is not logged in'];
    const a = setup(unavailable);
    const rec = await a.drafter.draft(input);
    expect(rec).toMatchObject({ status: 'failed', draft: null });
    expect(rec.error).toMatch(/cannot draft here: claude is not logged in/);
    expect(a.errors).toHaveLength(1);

    const noConfig = setup(new ScriptedAgent([writes('none')]));
    delete noConfig.git.files['ratchet.yaml'];
    expect((await noConfig.drafter.draft(input)).error).toMatch(/ratchet\.yaml not found/);

    const silent = setup(new ScriptedAgent([() => 'I looked around.']));
    expect((await silent.drafter.draft(input)).error).toMatch(/did not say which scope/);

    await expect(setup(new ScriptedAgent([writes('none')])).drafter.draft({ ...input, goal: ' ' })).rejects.toMatchObject({ code: 'invalid_input' });
  });
});

describe('AgentEnvironmentDrafter.revise', () => {
  it('resumes the same agent session with the user message, re-validates, accumulates effort, keeps the old draft when the agent fails', async () => {
    const agent = new ScriptedAgent([writes('full'), writes('single', 'smaller now'), () => { throw new Error('agent crashed'); }]);
    const { drafter } = setup(agent, { checker: passingChecker() });
    const first = await drafter.draft(input);
    const second = await drafter.revise(first.id, 'make it one machine');
    expect(agent.resumed).toEqual([{ id: 'sess-1', kind: 'claude-code' }]);
    expect(agent.prompts[1]).toContain('make it one machine');
    expect(second).toMatchObject({ status: 'ready', revision: 2 });
    expect(second.draft).toMatchObject({ scope: 'single', reason: 'smaller now', usdSpent: 1 });
    expect(second.messages.map((m) => m.from)).toEqual(['user', 'agent', 'user', 'agent']);

    const third = await drafter.revise(first.id, 'break');
    expect(third).toMatchObject({ status: 'ready', revision: 2, error: 'agent crashed' });
    expect(third.draft!.scope).toBe('single'); // the earlier draft is still there
    expect(third.messages.at(-1)!.text).toMatch(/could not make that change: agent crashed/);
  });

  it('refuses an empty message, an unknown draft, a failed draft, and an approved one', async () => {
    const { drafter } = setup(new ScriptedAgent([writes('single')]));
    const rec = await drafter.draft(input);
    await expect(drafter.revise(rec.id, '  ')).rejects.toMatchObject({ code: 'invalid_input' });
    await expect(drafter.revise('nope', 'x')).rejects.toMatchObject({ code: 'not_found' });
    await drafter.approve(rec.id);
    await expect(drafter.revise(rec.id, 'x')).rejects.toMatchObject({ code: 'invalid_state', message: expect.stringMatching(/locked/) });
    const failed = setup(new ScriptedAgent([() => 'nothing']));
    const f = await failed.drafter.draft(input);
    await expect(failed.drafter.revise(f.id, 'x')).rejects.toMatchObject({ code: 'invalid_state' });
  });
});

describe('AgentEnvironmentDrafter.approve', () => {
  const ready = async (agent = new ScriptedAgent([writes('full')])): Promise<{ s: ReturnType<typeof setup>; rec: EnvironmentDraftRecord }> => {
    const s = setup(agent, { checker: passingChecker() });
    return { s, rec: await s.drafter.draft(input) };
  };

  it('opens a pull request with the draft files by default, locks the record, and releases the scratch checkout', async () => {
    const { s, rec } = await ready();
    const done = await s.drafter.approve(rec.id);
    expect(done).toMatchObject({ status: 'approved', approval: { mode: 'pr', locked: true, branch: 'ratchet/environment-0001' } });
    expect(done.approval!.prUrl).toMatch(/^https:\/\//);
    expect(s.git.filesCommitted[0]!.input.files.map((f) => f.path)).toEqual(expect.arrayContaining(['ratchet.yaml', 'infra/ratchet/main.tf']));
    expect(s.git.prs[0]!.title).toContain('full-stack');
    expect(await s.drafter.approve(rec.id)).toBe(done); // idempotent, no second PR
    expect(s.git.prs).toHaveLength(1);
  });

  it('mode commit writes straight onto the base ref (no pull request)', async () => {
    const { s, rec } = await ready();
    const done = await s.drafter.approve(rec.id, { mode: 'commit' });
    expect(done.approval).toMatchObject({ mode: 'commit', locked: true });
    expect(s.git.filesCommitted[0]!.input).toMatchObject({ branch: 'main', update: true });
    expect(s.git.prs).toHaveLength(0);
  });

  it('refuses a draft whose checks did not all pass (no checker -> the plan did not run)', async () => {
    const s = setup(new ScriptedAgent([writes('full')]));
    const rec = await s.drafter.draft(input);
    await expect(s.drafter.approve(rec.id)).rejects.toMatchObject({ code: 'invalid_state', message: expect.stringMatching(/Plans cleanly/) });
    expect(s.git.filesCommitted).toHaveLength(0);
    expect(rec.status).toBe('ready');
  });

  it('refuses when ratchet.yaml changed on the base ref since the draft was made', async () => {
    const { s, rec } = await ready();
    s.git.files['ratchet.yaml'] += '# edited meanwhile\n';
    await expect(s.drafter.approve(rec.id)).rejects.toMatchObject({ code: 'invalid_state', message: expect.stringMatching(/changed on main since this environment was drafted/) });
    expect(s.git.filesCommitted).toHaveLength(0);
  });

  it('an approved draft cannot be discarded; a ready one can', async () => {
    const { s, rec } = await ready();
    s.drafter.discard(rec.id);
    expect(s.drafter.get(rec.id)).toBeUndefined();
    const r2 = await s.drafter.draft(input);
    await s.drafter.approve(r2.id);
    expect(() => s.drafter.discard(r2.id)).toThrow(/approved/);
  });
});
