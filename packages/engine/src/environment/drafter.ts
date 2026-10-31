import { createHash, randomBytes } from 'node:crypto';
import { lstat, mkdir, mkdtemp, readdir, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  DEFAULT_ENVIRONMENT_INFRA_DIR,
  applyEnvironmentToConfigYaml,
  buildReviseBrief,
  buildSetupBrief,
  estimateEnvironmentCostLive,
  parseAgentAnswer,
  parseConfig,
  readEnvironmentScope,
  validateEnvironment,
  type AgentAdapter,
  type AgentEvent,
  type AgentKind,
  type AgentSession,
  type ApproveEnvironmentOptions,
  type Clock,
  type DraftEnvironmentInput,
  type EnvironmentCheck,
  type EnvironmentChecker,
  type EnvironmentDraft,
  type EnvironmentDraftRecord,
  type EnvironmentDrafter,
  type EnvironmentFile,
  type EnvironmentScope,
  type GitHost,
  type PriceBook,
} from '@ratchet/core';
import { readEnvironmentTree } from '../environment-files.js';
import { EngineError } from '../errors.js';
import { DEFAULT_TOKEN_PRICING } from '../settings.js';
import { commitDraft } from './approve.js';

export interface AgentEnvironmentDrafterOptions {
  /** The git host of a repository (one host, or a factory when the app drives several). */
  git: GitHost | ((repoUrl: string) => GitHost);
  /** The user's connector: the adapter of their own Claude Code / Codex login, run locally (`localAdapterFactory` in @ratchet/agents). Throws when the agent cannot run here. */
  adapterFor(kind: AgentKind): AgentAdapter;
  clock: Clock;
  /** Checks the draft's OpenTofu against the user's AWS account. */
  checker?: EnvironmentChecker;
  /** Live prices (the AWS price book); the static table is the fallback. */
  prices?: PriceBook;
  /** Where the setup agent's checkout lives (default: the OS temp dir). */
  scratchDir?: string;
  /** Region when neither the input nor ratchet.yaml names one. Default `us-east-1`. */
  defaultRegion?: string;
  /** Used when the agent reports tokens but no cost. */
  tokenPricing?: { inputUsdPerMTok: number; outputUsdPerMTok: number };
  model?: string;
  maxTurns?: number;
  /** The agent is aborted after this long (default 15 min). */
  agentTimeoutMs?: number;
  newId?: () => string;
  onError?: (err: unknown, context: string) => void;
}

interface Work {
  dir: string;
  session: AgentSession;
  adapter: AgentAdapter;
  kind: AgentKind;
  originalConfigYaml: string;
  configPath: string;
  timeLimitSeconds: number;
  awsProfile: string | undefined;
  baseSha: string;
}

const SKIP_DIRS = new Set(['.git', 'node_modules', 'dist', 'build', '.next', 'target', '.venv', '__pycache__']);

/** path -> content hash of the text files of a tree (what the agent was allowed to change is read separately). */
async function digest(dir: string): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  const walk = async (rel: string): Promise<void> => {
    if (out.size > 20_000) return;
    for (const name of await readdir(join(dir, rel))) {
      const r = rel ? `${rel}/${name}` : name;
      const st = await lstat(join(dir, r));
      if (st.isDirectory()) {
        if (!SKIP_DIRS.has(name)) await walk(r);
      } else if (st.isFile() && st.size <= 1_000_000) {
        out.set(r, createHash('sha1').update(await readFile(join(dir, r))).digest('hex'));
      }
    }
  };
  await walk('');
  return out;
}

const textOf = (e: AgentEvent): string => (e.type === 'message' && e.text ? e.text : '');

/**
 * The real drafter: a setup agent (the user's agent connector, local) reads the repo and the goal and writes only the
 * environment (`environment:` in ratchet.yaml and files under `infra/ratchet/`); Ratchet validates it against the sandbox
 * contract (`validateEnvironment`), runs `tofu plan` in the user's account (`checker`), and prices it. `revise` resumes the same
 * agent session with the user's change request; `approve` writes the files to the protected ref (commit or pull request).
 * The drafts live in memory: they are a conversation, not state worth surviving a restart (the approved files live in the repo).
 */
export class AgentEnvironmentDrafter implements EnvironmentDrafter {
  private readonly records = new Map<string, EnvironmentDraftRecord>();
  private readonly work = new Map<string, Work>();
  private readonly busy = new Set<string>();

  constructor(private readonly o: AgentEnvironmentDrafterOptions) {}

  private gitFor(repo: string): GitHost {
    return typeof this.o.git === 'function' ? this.o.git(repo) : this.o.git;
  }

  get(draftId: string): EnvironmentDraftRecord | undefined {
    return this.records.get(draftId);
  }

  list(): EnvironmentDraftRecord[] {
    return [...this.records.values()].sort((a, b) => b.createdAt - a.createdAt);
  }

  discard(draftId: string): void {
    const rec = this.records.get(draftId);
    if (rec?.status === 'approved') throw new EngineError('invalid_state', `draft ${draftId} is approved and cannot be discarded`);
    void this.release(draftId);
    this.records.delete(draftId);
  }

  private async release(draftId: string): Promise<void> {
    const w = this.work.get(draftId);
    this.work.delete(draftId);
    if (w) await rm(w.dir, { recursive: true, force: true }).catch(() => undefined);
  }

  private require(draftId: string): EnvironmentDraftRecord {
    const rec = this.records.get(draftId);
    if (!rec) throw new EngineError('not_found', `environment draft ${draftId} not found`);
    return rec;
  }

  // ------------------------------------------------------------------ draft

  async draft(input: DraftEnvironmentInput): Promise<EnvironmentDraftRecord> {
    const bad = (m: string): never => {
      throw new EngineError('invalid_input', m);
    };
    if (!input.repo) bad('repo is required');
    if (!input.goal || input.goal.trim() === '') bad('goal is required');
    if (input.agentConnector !== 'claude-code' && input.agentConnector !== 'codex' && input.agentConnector !== 'mock') bad(`unknown agent connector ${JSON.stringify(input.agentConnector)}`);
    const now = this.o.clock.now();
    const id = this.o.newId?.() ?? `env-${randomBytes(4).toString('hex')}`;
    const configPath = input.configPath ?? 'ratchet.yaml';
    const baseRef = input.baseRef ?? 'main';
    const rec: EnvironmentDraftRecord = {
      id,
      repo: input.repo,
      baseRef,
      goal: input.goal.trim(),
      ...(input.awsProfile ? { awsProfile: input.awsProfile } : {}),
      region: input.region ?? this.o.defaultRegion ?? 'us-east-1',
      status: 'drafting',
      revision: 0,
      draft: null,
      messages: [{ from: 'user', text: input.goal.trim(), at: now }],
      createdAt: now,
      updatedAt: now,
    };
    this.records.set(id, rec);
    const progress = (phase: 'reading' | 'drafting' | 'validating' | 'done' | 'failed', text: string): void => input.onProgress?.({ draftId: id, phase, text });

    try {
      progress('reading', 'reading the repository');
      const git = this.gitFor(input.repo);
      const baseSha = await git.resolveRef(baseRef);
      const scratch = this.o.scratchDir ?? tmpdir();
      await mkdir(scratch, { recursive: true });
      const dir = await mkdtemp(join(scratch, 'ratchet-env-draft-'));
      await git.checkout(baseSha, dir);
      const originalConfigYaml = await readFile(join(dir, configPath), 'utf8').catch(() => {
        throw new EngineError('invalid_config', `${configPath} not found on ${baseRef}: add a ratchet.yaml (or use a template) before drafting an environment`);
      });
      const parsed = parseConfig(originalConfigYaml);
      const hardware = parsed.ok ? parsed.config.evaluator.hardware : undefined;
      if (parsed.ok && parsed.config.environment?.region && !input.region) rec.region = parsed.config.environment.region;
      const adapter = this.o.adapterFor(input.agentConnector);
      const pre = await adapter.preflight();
      if (!pre.ok) throw new EngineError('unavailable', `${input.agentConnector} cannot draft here: ${pre.problems.join('; ')}`);

      const prompt = buildSetupBrief({ goal: input.goal, ...(hardware ? { hardware } : {}), ...(input.timeLimitSeconds ? { timeLimitSeconds: input.timeLimitSeconds } : {}) });
      progress('drafting', `${input.agentConnector} is designing the environment`);
      const started = this.o.clock.now();
      const before = await digest(dir);
      const run = await this.runAgent(adapter, undefined, { workdir: dir, prompt }, progress);
      const work: Work = {
        dir,
        session: run.session,
        adapter,
        kind: input.agentConnector,
        originalConfigYaml,
        configPath,
        timeLimitSeconds: input.timeLimitSeconds ?? 2 * 3600,
        awsProfile: input.awsProfile,
        baseSha,
      };
      this.work.set(id, work);
      progress('validating', 'checking the draft against the sandbox contract');
      const { draft, strays } = await this.assemble(rec, work, run, started, before);
      rec.draft = draft;
      rec.revision = 1;
      rec.status = 'ready';
      rec.messages.push({ from: 'agent', text: [draft.reason, strays.length > 0 ? `(The agent also changed ${strays.join(', ')}: those changes are not part of the draft and are never committed.)` : ''].filter(Boolean).join(' '), at: this.o.clock.now() });
    } catch (err) {
      rec.status = 'failed';
      rec.error = (err as Error).message;
      this.o.onError?.(err, `draft ${id}`);
      await this.release(id);
      progress('failed', rec.error);
    }
    rec.updatedAt = this.o.clock.now();
    if (rec.status === 'ready') progress('done', 'the draft is ready');
    return rec;
  }

  // ------------------------------------------------------------------ revise

  async revise(draftId: string, message: string): Promise<EnvironmentDraftRecord> {
    const rec = this.require(draftId);
    if (!message || message.trim() === '') throw new EngineError('invalid_input', 'say what to change');
    if (rec.status === 'approved') throw new EngineError('invalid_state', `draft ${draftId} is approved: the environment is locked; start a new draft to change it`);
    if (rec.status === 'drafting' || this.busy.has(draftId)) throw new EngineError('invalid_state', `draft ${draftId} is being drafted`);
    const w = this.work.get(draftId);
    if (!w || rec.status === 'failed') throw new EngineError('invalid_state', `draft ${draftId} has no agent session to continue (${rec.error ?? 'it failed'}); draft again`);
    this.busy.add(draftId);
    rec.messages.push({ from: 'user', text: message.trim(), at: this.o.clock.now() });
    const previous = rec.status;
    rec.status = 'drafting';
    try {
      const started = this.o.clock.now();
      const before = await digest(w.dir);
      const run = await this.runAgent(w.adapter, w.session, { workdir: w.dir, prompt: buildReviseBrief(message) });
      w.session = run.session;
      const { draft, strays } = await this.assemble(rec, w, run, started, before, rec.draft ?? undefined);
      rec.draft = draft;
      rec.revision += 1;
      rec.status = 'ready';
      rec.messages.push({ from: 'agent', text: [draft.reason, strays.length > 0 ? `(Also changed ${strays.join(', ')}: not part of the draft.)` : ''].filter(Boolean).join(' '), at: this.o.clock.now() });
    } catch (err) {
      rec.status = previous; // the earlier draft is still there
      rec.error = (err as Error).message;
      rec.messages.push({ from: 'agent', text: `I could not make that change: ${rec.error}`, at: this.o.clock.now() });
      this.o.onError?.(err, `revise ${draftId}`);
    } finally {
      this.busy.delete(draftId);
      rec.updatedAt = this.o.clock.now();
    }
    return rec;
  }

  // ------------------------------------------------------------------ approve

  async approve(draftId: string, options: ApproveEnvironmentOptions = {}): Promise<EnvironmentDraftRecord> {
    const rec = this.require(draftId);
    if (rec.status === 'approved') return rec;
    if (rec.status !== 'ready') throw new EngineError('invalid_state', `draft ${draftId} is ${rec.status}; only a ready draft can be approved`);
    const w = this.work.get(draftId);
    rec.approval = await commitDraft(this.gitFor(rec.repo), rec, {
      ...options,
      configPath: w?.configPath ?? 'ratchet.yaml',
      ...(w ? { expectedConfigYaml: w.originalConfigYaml } : {}),
      ...(this.o.scratchDir ? { scratchDir: this.o.scratchDir } : {}),
      now: () => this.o.clock.now(),
      branchSuffix: draftId.replace(/^env-/, ''),
    });
    rec.status = 'approved';
    rec.updatedAt = this.o.clock.now();
    await this.release(draftId);
    return rec;
  }

  // ------------------------------------------------------------------ internals

  private async runAgent(
    adapter: AgentAdapter,
    session: AgentSession | undefined,
    spec: { workdir: string; prompt: string },
    progress?: (phase: 'drafting', text: string) => void,
  ): Promise<{ session: AgentSession; text: string; usd: number }> {
    const text: string[] = [];
    let usd = 0;
    const onEvent = (e: AgentEvent): void => {
      const t = textOf(e);
      if (t) text.push(t);
      if (e.usage) usd += e.usage.costUsd ?? (e.usage.inputTokens * (this.o.tokenPricing ?? DEFAULT_TOKEN_PRICING).inputUsdPerMTok + e.usage.outputTokens * (this.o.tokenPricing ?? DEFAULT_TOKEN_PRICING).outputUsdPerMTok) / 1e6;
      if (e.type === 'tool' && e.text) progress?.('drafting', e.text);
    };
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), this.o.agentTimeoutMs ?? 15 * 60_000);
    const agentSpec = { workdir: spec.workdir, prompt: spec.prompt, ...(this.o.model ? { model: this.o.model } : {}), ...(this.o.maxTurns ? { maxTurns: this.o.maxTurns } : {}) };
    try {
      let s = session;
      if (s) await adapter.resume(s, agentSpec, spec.prompt, onEvent, abort.signal);
      else s = await adapter.start(agentSpec, onEvent, abort.signal);
      return { session: s, text: text.join('\n'), usd };
    } catch (err) {
      if (abort.signal.aborted) throw new EngineError('aborted', `the setup agent did not finish within ${Math.round((this.o.agentTimeoutMs ?? 15 * 60_000) / 60_000)} min`);
      throw err;
    } finally {
      clearTimeout(timer);
    }
  }

  /** Read what the agent wrote, validate, price: the draft the UI shows. */
  private async assemble(
    rec: EnvironmentDraftRecord,
    w: Work,
    run: { text: string; usd: number },
    started: number,
    before: Map<string, string>,
    previous?: EnvironmentDraft,
  ): Promise<{ draft: EnvironmentDraft; strays: string[] }> {
    const answer = parseAgentAnswer(run.text);
    let configYaml = await readFile(join(w.dir, w.configPath), 'utf8').catch(() => w.originalConfigYaml);
    const declared = readEnvironmentScope(configYaml);
    const scope: EnvironmentScope | undefined = answer?.scope ?? declared ?? previous?.scope;
    if (!scope) throw new EngineError('invalid_config', 'the setup agent did not say which scope the environment needs (none, single or full)');
    // Ratchet records the decision itself when the agent forgot to, or wrote a different one.
    if (declared !== scope) configYaml = applyEnvironmentToConfigYaml(configYaml, { scope });

    const infra = await readEnvironmentTree(join(w.dir, ...DEFAULT_ENVIRONMENT_INFRA_DIR.split('/'))).catch((err: NodeJS.ErrnoException) => {
      if (err.code === 'ENOENT') return [];
      throw new EngineError('invalid_config', `cannot read ${DEFAULT_ENVIRONMENT_INFRA_DIR}/: ${err.message}`);
    });
    const files: EnvironmentFile[] = [
      { path: w.configPath, content: configYaml },
      ...infra.map((f) => ({ path: `${DEFAULT_ENVIRONMENT_INFRA_DIR}/${f.path}`, content: f.content })),
    ];
    const v = validateEnvironment({ scope, files, originalConfigYaml: w.originalConfigYaml, configPath: w.configPath, timeLimitSeconds: w.timeLimitSeconds });
    let { resources, cost } = v;
    if (this.o.prices && v.machines.length > 0) {
      const live = await estimateEnvironmentCostLive(v.machines, { timeLimitSeconds: w.timeLimitSeconds }, this.o.prices);
      resources = live.resources;
      cost = live.cost;
    }

    const checks: EnvironmentCheck[] = v.checks.filter((c) => c.id !== 'plan');
    const hasTf = infra.some((f) => f.path.endsWith('.tf'));
    if (!hasTf) {
      checks.push({ id: 'plan', label: 'Plans cleanly in your AWS account', ok: true, detail: 'no OpenTofu to plan: the standard sandbox is used' });
    } else if (!v.ok) {
      checks.push({ id: 'plan', label: 'Plans cleanly in your AWS account', ok: false, detail: 'not run: fix the failed checks first' });
    } else if (!this.o.checker) {
      checks.push({ id: 'plan', label: 'Plans cleanly in your AWS account', ok: false, detail: 'not run: connect AWS (Settings) so Ratchet can plan this in your account' });
    } else {
      try {
        const r = await this.o.checker.plan({ scope, files: files.filter((f) => f.path.startsWith(`${DEFAULT_ENVIRONMENT_INFRA_DIR}/`)), region: rec.region, awsProfile: w.awsProfile });
        checks.push({ id: 'plan', label: 'Plans cleanly in your AWS account', ok: r.ok, detail: r.detail });
      } catch (err) {
        checks.push({ id: 'plan', label: 'Plans cleanly in your AWS account', ok: false, detail: `could not run tofu: ${(err as Error).message}` });
      }
    }

    const after = await digest(w.dir);
    const allowed = (p: string): boolean => p === w.configPath || p.startsWith(`${DEFAULT_ENVIRONMENT_INFRA_DIR}/`);
    const strays = [...new Set([...before.keys(), ...after.keys()])].filter((p) => !allowed(p) && before.get(p) !== after.get(p)).sort();

    const draft: EnvironmentDraft = {
      scope,
      reason: answer?.reason || previous?.reason || '',
      resources,
      files: files.sort((a, b) => (a.path === w.configPath ? -1 : b.path === w.configPath ? 1 : a.path < b.path ? -1 : 1)),
      checks,
      cost,
      agent: w.kind,
      durationS: Math.max(0, Math.round((this.o.clock.now() - started) / 1000)) + (previous?.durationS ?? 0),
      usdSpent: Math.round((run.usd + (previous?.usdSpent ?? 0)) * 10_000) / 10_000,
    };
    return { draft, strays };
  }
}
