/**
 * In-memory fakes of every port, scripted for engine tests. Not exported from the package.
 * All fakes append to one shared `log` so tests can assert cross-port ordering (kill switch).
 */
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type {
  AgentEvent,
  AgentIdentity,
  ArtifactStore,
  BenchJob,
  BenchLaunchSpec,
  BenchOutcome,
  BenchRun,
  BenchRunner,
  CandidateCommit,
  Clock,
  CloudCompute,
  GitHost,
  GuardrailResult,
  InstanceInfo,
  PlanChange,
  PriceBook,
  RatchetConfig,
  RemoteCommand,
  RemoteCommandResult,
  RunnerConfig,
  RunnerControl,
  RunnerHandle,
  RunnerLaunchSpec,
  RunnerMessage,
  SandboxInput,
  SandboxOutputs,
  SandboxProvider,
  SecretRef,
} from '@ratchet/core';

export class FakeClock implements Clock {
  constructor(public t = 1_700_000_000_000) {}
  now(): number {
    return this.t;
  }
  advance(ms: number): void {
    this.t += ms;
  }
  async sleep(ms: number, signal?: AbortSignal): Promise<void> {
    if (signal?.aborted) return;
    this.t += ms;
    await Promise.resolve();
  }
}

export type CallLog = string[];

// ---------------------------------------------------------------- git

export interface FakeCommit extends CandidateCommit {
  files: string[];
}

export class FakeGit implements GitHost {
  readonly branches = new Map<string, string>(); // branch -> fromRef
  readonly commits = new Map<string, FakeCommit[]>();
  readonly prs: Array<{ base: string; head: string; title: string; body: string; shas?: string[] }> = [];
  failChangedFiles = new Set<string>();
  files: Record<string, string>;

  constructor(
    private readonly log: CallLog,
    readonly clock: Clock,
    files: Record<string, string>,
  ) {
    this.files = files;
  }

  /** Script a candidate commit on `branch` (what the runner would push). */
  push(branch: string, sha: string, changed: string[], message = `perf: ${sha}\n\nRatchet-Candidate: true`): void {
    const list = this.commits.get(branch) ?? [];
    list.push({ sha, branch, message, committedAt: this.clock.now(), files: changed });
    this.commits.set(branch, list);
  }

  async createBranch(fromRef: string, branch: string): Promise<void> {
    this.log.push(`git.createBranch:${branch}`);
    this.branches.set(branch, fromRef);
  }

  async listCandidates(branch: string, sinceSha?: string): Promise<CandidateCommit[]> {
    const list = this.commits.get(branch) ?? [];
    const idx = sinceSha ? list.findIndex((c) => c.sha === sinceSha) : -1;
    return list.slice(idx + 1).map(({ files: _f, ...c }) => c);
  }

  async changedFiles(_baseRef: string, sha: string): Promise<string[]> {
    if (this.failChangedFiles.has(sha)) throw new Error(`cannot diff ${sha}`);
    for (const list of this.commits.values()) {
      const c = list.find((x) => x.sha === sha);
      if (c) return c.files;
    }
    throw new Error(`unknown sha ${sha}`);
  }

  async checkout(_sha: string, dir: string): Promise<void> {
    for (const [path, body] of Object.entries(this.files)) {
      const full = join(dir, path);
      await mkdir(dirname(full), { recursive: true });
      await writeFile(full, body);
    }
  }

  async openPullRequest(input: { base: string; head: string; title: string; body: string }): Promise<{ url: string }> {
    this.log.push('git.openPullRequest');
    this.prs.push(input as (typeof this.prs)[number]);
    return { url: `https://example.test/pr/${this.prs.length}` };
  }

  remoteUrl(): string {
    return 'https://example.test/acme/repo.git';
  }
}

// ---------------------------------------------------------------- bench

export interface ScriptedBench {
  samples: number[];
  guardrails?: GuardrailResult[];
  metrics?: Record<string, number>;
  /** Throw this many times before succeeding. */
  failTimes?: number;
}

export class FakeBench implements BenchRunner {
  readonly jobs: BenchJob[] = [];
  /** key `${sha}:${phase}`, or `base:${phase}` for the baseline sha. */
  readonly script = new Map<string, ScriptedBench>();
  /** When set, bench runs wait on this gate (to hold a job in flight). */
  gate?: Promise<void>;
  baselineArgs: Array<{ metrics: Record<string, number> } | null> = [];
  private readonly failed = new Map<string, number>();

  constructor(
    private readonly log: CallLog,
    private readonly clock: Clock,
    readonly baseRef = 'main',
  ) {}

  set(sha: string, phase: 'baseline' | 'bench' | 'confirm', s: ScriptedBench): void {
    this.script.set(`${sha}:${phase}`, s);
  }

  async run(job: BenchJob, _config: RatchetConfig, baseline: { metrics: Record<string, number> } | null, signal?: AbortSignal): Promise<BenchOutcome> {
    this.log.push(`bench.${job.phase}:${job.sha}`);
    this.jobs.push(job);
    this.baselineArgs.push(baseline);
    if (this.gate) {
      await Promise.race([
        this.gate,
        new Promise<never>((_, rej) => signal?.addEventListener('abort', () => rej(new Error('aborted')), { once: true })),
      ]);
    }
    if (signal?.aborted) throw new Error('aborted');
    const key = `${job.sha}:${job.phase}`;
    const s = this.script.get(key);
    if (!s) throw new Error(`no scripted bench result for ${key}`);
    if (s.failTimes) {
      const n = this.failed.get(key) ?? 0;
      if (n < s.failTimes) {
        this.failed.set(key, n + 1);
        throw new Error(`bench infrastructure hiccup #${n + 1}`);
      }
    }
    const startedAt = this.clock.now();
    const runs: BenchRun[] = s.samples.map((value) => ({ value, metrics: { ...(s.metrics ?? {}), objective: value }, durationMs: 1000 }));
    return { runs, samples: [...s.samples], guardrails: s.guardrails ?? [], startedAt, finishedAt: startedAt + 1000 };
  }
}

// ---------------------------------------------------------------- runners

export class FakeRunners implements RunnerControl {
  readonly started: Array<{ handle: RunnerHandle; config: RunnerConfig }> = [];
  readonly sent: Array<{ agentId: string; message: RunnerMessage }> = [];
  readonly stopped: string[] = [];
  readonly pollCalls: Array<{ agentId: string; cursor: string | undefined }> = [];
  readonly queues = new Map<string, AgentEvent[]>();
  failPoll = new Set<string>();
  failSend = new Set<string>();

  constructor(private readonly log: CallLog) {}

  /** Script events an agent's runner will report (appended after existing ones). */
  emit(agentId: string, ...events: AgentEvent[]): void {
    this.queues.set(agentId, [...(this.queues.get(agentId) ?? []), ...events]);
  }

  async start(handle: RunnerHandle, config: RunnerConfig): Promise<void> {
    this.log.push(`runners.start:${handle.agentId}`);
    this.started.push({ handle, config });
  }

  async send(handle: RunnerHandle, message: RunnerMessage): Promise<void> {
    this.log.push(`runners.send:${handle.agentId}:${message.type}`);
    if (this.failSend.has(handle.agentId)) throw new Error('ssm send failed');
    this.sent.push({ agentId: handle.agentId, message });
  }

  /** Cursor is the index into the agent's event list, as a string. */
  async poll(handle: RunnerHandle, cursor?: string): Promise<{ events: AgentEvent[]; cursor: string }> {
    this.pollCalls.push({ agentId: handle.agentId, cursor });
    if (this.failPoll.has(handle.agentId)) throw new Error('s3 unavailable');
    const all = this.queues.get(handle.agentId) ?? [];
    const from = cursor === undefined ? 0 : Number(cursor);
    return { events: all.slice(from), cursor: String(all.length) };
  }

  async stop(handle: RunnerHandle): Promise<void> {
    this.log.push(`runners.stop:${handle.agentId}`);
    this.stopped.push(handle.agentId);
  }

  sentTo(agentId: string): RunnerMessage[] {
    return this.sent.filter((s) => s.agentId === agentId).map((s) => s.message);
  }
}

// ---------------------------------------------------------------- compute

export class FakeCompute implements CloudCompute {
  readonly instances = new Map<string, InstanceInfo>();
  readonly identities = new Map<string, AgentIdentity & { revoked: boolean; deleted: boolean }>();
  readonly secrets = new Map<string, string>(); // `${opt}/${agent}/${name}` -> value
  readonly deletedSecretsFor = new Set<string>();
  /** Instances with these ids ignore terminate() (to prove orphan reporting). */
  stubborn = new Set<string>();
  failRevoke = new Set<string>();
  private n = 0;

  constructor(
    private readonly log: CallLog,
    private readonly clock: Clock,
  ) {}

  async createAgentIdentity(optimizationId: string, agentId: string): Promise<AgentIdentity> {
    this.log.push(`compute.createAgentIdentity:${agentId}`);
    const id = { optimizationId, agentId, roleRef: `role/${agentId}`, instanceProfileRef: `profile/${agentId}` };
    this.identities.set(agentId, { ...id, revoked: false, deleted: false });
    return id;
  }

  async revokeAgentIdentity(identity: AgentIdentity): Promise<void> {
    this.log.push(`compute.revoke:${identity.agentId}`);
    if (this.failRevoke.has(identity.agentId)) throw new Error('iam throttled');
    const cur = this.identities.get(identity.agentId);
    if (cur) cur.revoked = true;
  }

  async deleteAgentIdentity(identity: AgentIdentity): Promise<void> {
    this.log.push(`compute.deleteIdentity:${identity.agentId}`);
    const cur = this.identities.get(identity.agentId);
    if (cur) cur.deleted = true;
  }

  async putSecret(optimizationId: string, agentId: string, name: string, value: string): Promise<SecretRef> {
    this.log.push(`compute.putSecret:${agentId}:${name}`);
    this.secrets.set(`${optimizationId}/${agentId}/${name}`, value);
    return { name, ref: `/ratchet/${optimizationId}/${agentId}/${name}` };
  }

  async deleteSecrets(optimizationId: string, agentId: string): Promise<void> {
    this.log.push(`compute.deleteSecrets:${agentId}`);
    for (const k of [...this.secrets.keys()]) if (k.startsWith(`${optimizationId}/${agentId}/`)) this.secrets.delete(k);
    this.deletedSecretsFor.add(agentId);
  }

  private add(info: Omit<InstanceInfo, 'id' | 'state' | 'launchedAt'>): InstanceInfo {
    const inst: InstanceInfo = { ...info, id: `i-${String(++this.n).padStart(4, '0')}`, state: 'running', launchedAt: this.clock.now() };
    this.instances.set(inst.id, inst);
    return inst;
  }

  async launchRunner(spec: RunnerLaunchSpec): Promise<InstanceInfo> {
    this.log.push(`compute.launchRunner:${spec.agentId}`);
    return this.add({ role: 'agent_runner', optimizationId: spec.optimizationId, agentId: spec.agentId, instanceType: spec.instanceType });
  }

  async launchBenchInstance(spec: BenchLaunchSpec): Promise<InstanceInfo> {
    return this.add({ role: 'bench', optimizationId: spec.optimizationId, instanceType: spec.instanceType });
  }

  /** Test helper: an agent's experiment instance. */
  launchExperiment(optimizationId: string, agentId: string, instanceType = 'c7i.xlarge'): InstanceInfo {
    return this.add({ role: 'experiment', optimizationId, agentId, instanceType });
  }

  async terminate(instanceIds: string[]): Promise<void> {
    this.log.push(`compute.terminate:${instanceIds.join(',')}`);
    for (const id of instanceIds) {
      const i = this.instances.get(id);
      if (!i || this.stubborn.has(id)) continue;
      i.state = 'terminated';
      i.terminatedAt = this.clock.now();
    }
  }

  async listTagged(optimizationId?: string): Promise<InstanceInfo[]> {
    return [...this.instances.values()].filter((i) => !optimizationId || i.optimizationId === optimizationId).map((i) => ({ ...i }));
  }

  async runCommand(_id: string, _c: RemoteCommand): Promise<RemoteCommandResult> {
    return { exitCode: 0, stdout: '', stderr: '' };
  }

  live(optimizationId?: string): InstanceInfo[] {
    return [...this.instances.values()].filter((i) => i.state !== 'terminated' && (!optimizationId || i.optimizationId === optimizationId));
  }
}

export class FakePrices implements PriceBook {
  constructor(
    private readonly prices: Record<string, number> = {},
    private readonly fallback = 1,
  ) {}
  async hourlyUsd(instanceType: string): Promise<number> {
    return this.prices[instanceType] ?? this.fallback;
  }
}

export class FakeArtifacts implements ArtifactStore {
  readonly store = new Map<string, string | Uint8Array>();
  async put(key: string, body: string | Uint8Array): Promise<void> {
    this.store.set(key, body);
  }
  async get(key: string): Promise<string | null> {
    const v = this.store.get(key);
    return v === undefined ? null : typeof v === 'string' ? v : Buffer.from(v).toString('utf8');
  }
  async list(prefix: string): Promise<string[]> {
    return [...this.store.keys()].filter((k) => k.startsWith(prefix));
  }
}

export class FakeSandbox implements SandboxProvider {
  async plan(_i: SandboxInput): Promise<{ changes: PlanChange[]; summary: string }> {
    return { changes: [], summary: 'no changes' };
  }
  async apply(_i: SandboxInput): Promise<SandboxOutputs> {
    throw new Error('not used by the engine');
  }
  async destroy(_i: SandboxInput): Promise<void> {}
  async outputs(_id: string): Promise<SandboxOutputs | null> {
    return null;
  }
}
