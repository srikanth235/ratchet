import { randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import {
  validateEnvironment,
  type ApproveEnvironmentOptions,
  type Clock,
  type DraftEnvironmentInput,
  type EnvironmentApproval,
  type EnvironmentCheck,
  type EnvironmentDraft,
  type EnvironmentDraftRecord,
  type EnvironmentDrafter,
  type EnvironmentScope,
  type GitHost,
} from '@ratchet/core';
import { EngineError } from '../errors.js';
import { commitDraft } from './approve.js';
import { MOCK_CONFIG_YAML, SAMPLE_EFFORT, SAMPLE_REASONS, sampleEnvironment } from './samples.js';

export interface MockEnvironmentDrafterOptions {
  clock?: Clock;
  /** The scope every draft gets, or a function of the request. Default: chosen from the goal's words (`scopeFromGoal`). */
  scope?: EnvironmentScope | ((input: DraftEnvironmentInput) => EnvironmentScope);
  /** The repo's ratchet.yaml the drafts are made on (default: a minimal http-latency config). */
  configYaml?: string;
  /** Pretend drafting takes this long (a real `clock.sleep`), so a UI can show its progress. Default 0. */
  delayMs?: number;
  /**
   * With a git host, `approve` really commits (directly or as a PR) through `commitFiles`, exactly like the real drafter;
   * without one the approval is synthetic (a fake sha / PR url), for demos that have no repository.
   */
  git?: GitHost | ((repoUrl: string) => GitHost);
  newId?: () => string;
}

/**
 * A deterministic stand-in for the real drafter: no agent, no AWS, no network. Drafts are the example environments of
 * `sampleEnvironment`, validated by the real validator and priced by the real cost estimator, so everything the UI shows (the
 * rows, the checks, the cost) is computed, not canned. Used by simulation, the desktop's demo mode and the tests.
 */
export class MockEnvironmentDrafter implements EnvironmentDrafter {
  private readonly records = new Map<string, EnvironmentDraftRecord>();
  /** The ratchet.yaml each draft was made on. */
  private readonly originals = new Map<string, string>();
  private seq = 0;

  constructor(private readonly o: MockEnvironmentDrafterOptions = {}) {}

  private now(): number {
    return this.o.clock?.now() ?? Date.now();
  }

  private gitFor(repo: string): GitHost | undefined {
    return typeof this.o.git === 'function' ? this.o.git(repo) : this.o.git;
  }

  get(draftId: string): EnvironmentDraftRecord | undefined {
    return this.records.get(draftId);
  }

  list(): EnvironmentDraftRecord[] {
    return [...this.records.values()].sort((a, b) => b.createdAt - a.createdAt);
  }

  discard(draftId: string): void {
    if (this.records.get(draftId)?.status === 'approved') throw new EngineError('invalid_state', `draft ${draftId} is approved and cannot be discarded`);
    this.records.delete(draftId);
  }

  private pickScope(input: DraftEnvironmentInput): EnvironmentScope {
    const s = this.o.scope;
    return typeof s === 'function' ? s(input) : (s ?? scopeFromGoal(input.goal));
  }

  async draft(input: DraftEnvironmentInput): Promise<EnvironmentDraftRecord> {
    if (!input.repo) throw new EngineError('invalid_input', 'repo is required');
    if (!input.goal || input.goal.trim() === '') throw new EngineError('invalid_input', 'goal is required');
    const t = this.now();
    const id = this.o.newId?.() ?? `env-${randomBytes(4).toString('hex')}`;
    const rec: EnvironmentDraftRecord = {
      id,
      repo: input.repo,
      baseRef: input.baseRef ?? 'main',
      goal: input.goal.trim(),
      ...(input.awsProfile ? { awsProfile: input.awsProfile } : {}),
      region: input.region ?? 'us-east-1',
      status: 'drafting',
      revision: 0,
      draft: null,
      messages: [{ from: 'user', text: input.goal.trim(), at: t }],
      createdAt: t,
      updatedAt: t,
    };
    this.records.set(id, rec);
    const progress = (phase: 'reading' | 'drafting' | 'validating' | 'done', text: string): void => input.onProgress?.({ draftId: id, phase, text });
    progress('reading', 'reading the repository');
    progress('drafting', `${input.agentConnector} is designing the environment`);
    if (this.o.delayMs) await this.o.clock?.sleep(this.o.delayMs);
    progress('validating', 'checking the draft against the sandbox contract');
    const scope = this.pickScope(input);
    this.originals.set(id, await this.originalConfig(rec, input.configPath ?? 'ratchet.yaml'));
    rec.draft = this.make(rec, scope, SAMPLE_REASONS[scope], input.agentConnector, input.timeLimitSeconds);
    rec.revision = 1;
    rec.status = 'ready';
    rec.messages.push({ from: 'agent', text: rec.draft.reason, at: this.now() });
    rec.updatedAt = this.now();
    progress('done', 'the draft is ready');
    return rec;
  }

  async revise(draftId: string, message: string): Promise<EnvironmentDraftRecord> {
    const rec = this.records.get(draftId);
    if (!rec) throw new EngineError('not_found', `environment draft ${draftId} not found`);
    if (!message || message.trim() === '') throw new EngineError('invalid_input', 'say what to change');
    if (rec.status === 'approved') throw new EngineError('invalid_state', `draft ${draftId} is approved: the environment is locked; start a new draft to change it`);
    if (!rec.draft) throw new EngineError('invalid_state', `draft ${draftId} has nothing to revise`);
    rec.messages.push({ from: 'user', text: message.trim(), at: this.now() });
    if (this.o.delayMs) await this.o.clock?.sleep(this.o.delayMs);
    const current = rec.draft;
    const m = message.toLowerCase();
    let scope = current.scope;
    let reason: string;
    if (/(smaller|simpler|cheaper|less|single|one machine|fewer)/.test(m)) {
      scope = current.scope === 'full' ? 'single' : 'none';
      reason = scope === current.scope ? `Already as small as it gets (${scope}).` : `Smaller setup: scope ${scope}. ${SAMPLE_REASONS[scope]}`;
    } else if (/(bigger|full stack|more realistic|add a database|postgres|redis|load generator)/.test(m) && !/rds/.test(m)) {
      scope = current.scope === 'none' ? 'single' : 'full';
      reason = scope === current.scope ? `Already the full stack (${scope}).` : `Larger setup: scope ${scope}. ${SAMPLE_REASONS[scope]}`;
    } else if (/\b(rds|aurora|managed database)\b/.test(m)) {
      reason = `${SAMPLE_REASONS[scope]} I kept Postgres in a container: a managed database (RDS) needs subnets in two zones, and the isolated sandbox has one private subnet with no route out.`;
    } else {
      reason = `${current.reason} Noted: "${message.trim()}". I kept the setup as it is because it already covers that.`;
    }
    rec.draft = this.make(rec, scope, reason, current.agent, undefined, current);
    rec.revision += 1;
    rec.status = 'ready';
    rec.messages.push({ from: 'agent', text: reason, at: this.now() });
    rec.updatedAt = this.now();
    return rec;
  }

  async approve(draftId: string, options: ApproveEnvironmentOptions = {}): Promise<EnvironmentDraftRecord> {
    const rec = this.records.get(draftId);
    if (!rec) throw new EngineError('not_found', `environment draft ${draftId} not found`);
    if (rec.status === 'approved') return rec;
    if (rec.status !== 'ready' || !rec.draft) throw new EngineError('invalid_state', `draft ${draftId} is ${rec.status}; only a ready draft can be approved`);
    const git = this.gitFor(rec.repo);
    if (git) {
      rec.approval = await commitDraft(git, rec, { ...options, configPath: 'ratchet.yaml', now: () => this.now(), branchSuffix: draftId.replace(/^env-/, '') });
    } else {
      const failed = rec.draft.checks.filter((c) => !c.ok);
      if (failed.length > 0) throw new EngineError('invalid_state', `draft ${draftId} cannot be approved: these checks did not pass: ${failed.map((c) => c.label).join(', ')}`);
      const mode = options.mode ?? 'pr';
      const n = ++this.seq;
      const sha = `mock${String(n).padStart(4, '0')}${draftId.replace(/\W/g, '').slice(0, 8)}`;
      const approval: EnvironmentApproval = {
        mode,
        sha,
        ...(mode === 'pr' ? { branch: `ratchet/environment-${draftId.replace(/^env-/, '')}`, prUrl: `https://example.test/ratchet/pull/${n}` } : {}),
        files: rec.draft.files.map((f) => f.path),
        locked: true,
        at: this.now(),
      };
      rec.approval = approval;
    }
    rec.status = 'approved';
    rec.updatedAt = this.now();
    return rec;
  }

  /** The repo's ratchet.yaml: the option, else the one on the base ref when there is a host, else a minimal config. */
  private async originalConfig(rec: EnvironmentDraftRecord, configPath: string): Promise<string> {
    if (this.o.configYaml !== undefined) return this.o.configYaml;
    const host = this.gitFor(rec.repo);
    if (!host) return MOCK_CONFIG_YAML;
    const scratch = join(tmpdir(), 'ratchet-mock-env');
    await mkdir(scratch, { recursive: true });
    const dir = await mkdtemp(join(scratch, 'read-'));
    try {
      await host.checkout(rec.baseRef, dir);
      return await readFile(join(dir, configPath), 'utf8');
    } catch {
      return MOCK_CONFIG_YAML;
    } finally {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    }
  }

  /** Build the draft for `scope` through the real validator and estimator. */
  private make(rec: EnvironmentDraftRecord, scope: EnvironmentScope, reason: string, agent: EnvironmentDraft['agent'], timeLimitSeconds?: number, previous?: EnvironmentDraft): EnvironmentDraft {
    const original = this.originals.get(rec.id) ?? this.o.configYaml ?? MOCK_CONFIG_YAML;
    const sample = sampleEnvironment(scope, { originalConfigYaml: original, reason });
    const v = validateEnvironment({
      scope,
      files: sample.files,
      originalConfigYaml: original,
      timeLimitSeconds: timeLimitSeconds ?? 2 * 3600,
    });
    const plan: EnvironmentCheck = {
      id: 'plan',
      label: 'Plans cleanly in your AWS account',
      ok: true,
      detail: sample.files.some((f) => f.path.endsWith('.tf')) ? 'simulated: no AWS account is touched in demo mode' : 'no OpenTofu to plan: the standard sandbox is used',
    };
    const effort = SAMPLE_EFFORT[scope];
    return {
      scope,
      reason,
      resources: v.resources,
      files: sample.files,
      checks: [...v.checks, plan],
      cost: v.cost,
      agent,
      durationS: effort.durationS + (previous?.durationS ?? 0),
      usdSpent: Math.round((effort.usdSpent + (previous?.usdSpent ?? 0)) * 100) / 100,
    };
  }
}

/** The scope a goal's words suggest (the mock's default chooser): databases and load -> full, tests -> none, otherwise one machine. */
export function scopeFromGoal(goal: string): EnvironmentScope {
  const g = goal.toLowerCase();
  if (/(p9\d|latency|throughput|checkout|database|postgres|redis|queries|requests?\/s|rps|load test|api|server|stack)/.test(g)) return 'full';
  if (/(unit test|tests?\b|lint|type ?check|compile|typecheck|refactor|dead code|coverage)/.test(g)) return 'none';
  return 'single';
}
