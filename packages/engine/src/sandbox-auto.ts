/**
 * The invisible sandbox (docs/SIMPLIFIED_MODEL.md, "Sandbox lifecycle: invisible").
 *
 *  - Created lazily at the first run start in a region (`ensureFor`), from `ratchet.yaml` (`environment.region`) or the default.
 *  - Reused while runs are active; every `tick()` looks at it.
 *  - Destroyed automatically once no run is active for the idle window (default 30 min): ended runs are torn down first
 *    (identities, secrets, leftover instances), the destroy goes through the same orphan check as an explicit one, and a
 *    `SandboxTeardownReceipt` is recorded: what was removed, what it cost, proof that nothing is left running.
 *
 * Only sandboxes this class created are destroyed automatically; one applied explicitly (`applySandbox`) stays until
 * `destroySandbox`. The idle clock is persisted, so a sandbox that outlived the app is destroyed on the first tick after launch.
 */
import { environmentSandboxId } from '@ratchet/core';
import type {
  CloudCompute,
  Clock,
  EngineEvent,
  Ledger,
  Mode,
  OptimizationRow,
  RatchetConfig,
  SandboxActivity,
  SandboxEnvironment,
  SandboxInput,
  SandboxLifecycle,
  SandboxOutputs,
  SandboxPhase,
  SandboxProvider,
  SandboxTeardownReceipt,
} from '@ratchet/core';
import type { TeardownReport } from './engine.js';
import { EngineError } from './errors.js';
import { K, type KvStore } from './kv.js';
import type { ResolvedSettings } from './settings.js';

const ACTIVE = new Set(['baselining', 'running', 'stopping']);
const ENDED = new Set(['stopped', 'failed']);

export interface AutoSandboxHost {
  ledger: Ledger;
  kv: KvStore;
  clock: Clock;
  mode: Mode;
  settings: ResolvedSettings;
  lifecycle: SandboxLifecycle;
  provider: SandboxProvider;
  compute: CloudCompute;
  configOf(optimizationId: string): RatchetConfig;
  /**
   * The environment this optimization is measured in, read from the protected ref (docs/SIMPLIFIED_MODEL.md, "Dynamic
   * environments"): the approved drafted environment, or the custom `infra:` module. `undefined` for scope `none` (and `single`
   * without OpenTofu): the standard sandbox, nothing extra. Throws `EngineError('invalid_config')` when the config asks for an
   * environment the protected ref does not hold, or one that fails the sandbox contract.
   */
  environmentOf(optimizationId: string): Promise<SandboxEnvironment | undefined>;
  /** The engine's `teardown`: secrets, identities, leftover instances; marks the optimization `completed`. */
  teardown(optimizationId: string): Promise<void>;
  teardownReport(optimizationId: string): TeardownReport | undefined;
  /** Runs a lifecycle call and maps its errors to `EngineError`. */
  lifecycleCall<T>(fn: () => Promise<T>): Promise<T>;
  /** True while some `start()` is between its first line and its end (a sandbox may be about to be needed). */
  hasPendingStarts(): boolean;
  emit(e: EngineEvent): void;
  reportError(err: unknown, context: string): void;
}

/** `environment.region` of ratchet.yaml. */
export function regionOf(config: RatchetConfig, fallback: string): string {
  // TODO(environment): the config schema is gaining an `environment` block (image, machine, region, network); until it lands
  // read it defensively and fall back to the setting default.
  const region = (config as unknown as { environment?: { region?: unknown } }).environment?.region;
  return typeof region === 'string' && region.trim() !== '' ? region.trim() : fallback;
}

export class AutoSandbox {
  /** Serializes ensure / destroy per sandbox (one apply or destroy at a time; the lifecycle refuses concurrent ones). */
  private readonly chains = new Map<string, Promise<unknown>>();
  /** After a failed automatic destroy, do not retry before this time. */
  private readonly retryAfter = new Map<string, number>();

  constructor(private readonly h: AutoSandboxHost) {}

  get enabled(): boolean {
    return this.h.settings.sandbox.auto;
  }

  sandboxIdFor(region: string): string {
    const s = this.h.settings.sandbox;
    if (s.sandboxId) return s.sandboxId(region);
    const base = this.h.mode === 'live' ? 'ratchet-sandbox' : 'ratchet-sim';
    return region === s.defaultRegion ? base : `${base}-${region}`;
  }

  private queue<T>(sandboxId: string, fn: () => Promise<T>): Promise<T> {
    const prev = this.chains.get(sandboxId) ?? Promise.resolve();
    const p = prev.then(fn, fn);
    const tail = p.catch(() => undefined);
    this.chains.set(sandboxId, tail);
    void tail.then(() => {
      if (this.chains.get(sandboxId) === tail) this.chains.delete(sandboxId);
    });
    return p;
  }

  private phase(sandboxId: string, phase: SandboxPhase, region: string | null, text: string): void {
    this.h.emit({ type: 'sandbox', sandboxId, phase, region, text });
  }

  // ------------------------------------------------------------------ create (first run start in a region)

  /**
   * Make sure the optimization's sandbox is applied, creating it if needed, and link the optimization to it. The idle clock
   * stops. Throws `EngineError('sandbox_unavailable')` when the apply fails (the optimization stays a draft).
   */
  async ensureFor(optimizationId: string): Promise<string> {
    const { ledger, kv, settings } = this.h;
    const opt = ledger.getOptimization(optimizationId);
    if (!opt) throw new EngineError('not_found', `optimization ${optimizationId} not found`);
    const config = this.h.configOf(optimizationId);
    const existing = opt.sandboxId ? ledger.getSandbox(opt.sandboxId) : undefined;
    const region = existing?.region ?? regionOf(config, settings.sandbox.defaultRegion);
    // The environment (if any) comes from the PROTECTED ref, never a candidate. Optimizations with the same region and the same
    // environment share a sandbox; a different environment gets a sandbox of its own (the id carries the content hash).
    const environment = existing?.status === 'applied' ? undefined : await this.h.environmentOf(optimizationId);
    const sandboxId = opt.sandboxId ?? (environment ? environmentSandboxId(region, environment.hash) : this.sandboxIdFor(region));

    await this.queue(sandboxId, async () => {
      const row = ledger.getSandbox(sandboxId);
      if (row?.status !== 'applied') {
        const input = this.inputFor(sandboxId, region, config, environment);
        const what = environment ? `the ${environment.scope === 'full' ? 'full-stack ' : ''}environment` : 'the environment';
        this.phase(sandboxId, 'creating', region, `creating ${what} in ${region}`);
        try {
          await this.h.lifecycleCall(() => this.h.lifecycle.apply(input));
        } catch (err) {
          this.phase(sandboxId, 'failed', region, `could not create the environment: ${(err as Error).message}`);
          throw new EngineError('sandbox_unavailable', `could not create the environment in ${region}: ${(err as Error).message}`, err);
        }
        kv.set(K.sandboxAuto(sandboxId), '1'); // ours: destroyed automatically after the idle window
        this.phase(sandboxId, 'ready', region, 'environment ready');
      }
      kv.delete(K.sandboxIdle(sandboxId));
    });
    if (opt.sandboxId !== sandboxId) ledger.setOptimizationSandbox(optimizationId, sandboxId);
    return sandboxId;
  }

  private inputFor(sandboxId: string, region: string, config: RatchetConfig, environment?: SandboxEnvironment): SandboxInput {
    const custom = this.h.settings.sandbox.input?.({ sandboxId, region, config });
    const base =
      custom ??
      ({
        sandboxId,
        region,
        allowedInstanceTypes: [...new Set([...config.agent.allowed_instance_types, config.bench.hardware])],
        maxInstancesPerAgent: 2,
        maxInstanceLifetimeHours: 4,
        tags: { 'ratchet:managed': 'true' },
      } satisfies SandboxInput);
    return environment ? { ...base, environment, tags: { ...base.tags, 'ratchet:env': environment.hash } } : base;
  }

  // ------------------------------------------------------------------ idle clock + destroy

  /** Called by the engine's `tick()`. Never throws. */
  async tick(): Promise<void> {
    if (!this.enabled) return;
    for (const row of this.h.ledger.listSandboxes()) {
      // `failed` + ours: a destroy that left infrastructure of unknown state; keep retrying it (with back-off).
      if ((row.status !== 'applied' && row.status !== 'failed') || !this.h.kv.has(K.sandboxAuto(row.id))) continue;
      try {
        await this.tickOne(row.id);
      } catch (err) {
        this.h.reportError(err, `auto sandbox ${row.id}`);
      }
    }
  }

  private activeRuns(sandboxId: string): OptimizationRow[] {
    return this.h.ledger.optimizationsInSandbox(sandboxId).filter((o) => ACTIVE.has(o.status));
  }

  /** When the idle window started: persisted at the first observation, derived from the last run's end (or the apply) until then. */
  private idleSince(sandboxId: string): number {
    const { kv, ledger, clock } = this.h;
    const known = kv.getNumber(K.sandboxIdle(sandboxId));
    if (known !== undefined) return known;
    const row = ledger.getSandbox(sandboxId);
    const ends = ledger.optimizationsInSandbox(sandboxId).map((o) => o.endedAt ?? o.createdAt);
    const since = Math.min(clock.now(), Math.max(row?.appliedAt ?? 0, ...ends));
    kv.set(K.sandboxIdle(sandboxId), String(since));
    return since;
  }

  private async tickOne(sandboxId: string): Promise<void> {
    const { kv, clock, settings } = this.h;
    if (this.activeRuns(sandboxId).length > 0 || this.h.hasPendingStarts()) {
      kv.delete(K.sandboxIdle(sandboxId));
      return;
    }
    const first = !kv.has(K.sandboxIdle(sandboxId));
    const since = this.idleSince(sandboxId);
    if (first) this.phase(sandboxId, 'idle', this.h.ledger.getSandbox(sandboxId)?.region ?? null, `no run is active; the environment is removed in ${Math.round(settings.sandbox.idleMs / 60_000)} min unless a run starts`);
    if (clock.now() - since < settings.sandbox.idleMs) return;
    if (clock.now() < (this.retryAfter.get(sandboxId) ?? 0)) return;
    try {
      await this.destroy(sandboxId, 'idle');
    } catch (err) {
      this.retryAfter.set(sandboxId, clock.now() + settings.sandbox.retryMs);
      this.h.reportError(err, `auto destroy ${sandboxId}`);
    }
  }

  /**
   * Tear the sandbox down now: end every finished run's cloud resources, destroy, verify, and record the receipt. Refuses while
   * a run is active. `trigger: 'idle'` is the automatic path; `'manual'` is `Engine.endSandbox`.
   */
  destroy(sandboxId: string, trigger: SandboxTeardownReceipt['trigger']): Promise<SandboxTeardownReceipt> {
    return this.queue(sandboxId, () => this.destroyNow(sandboxId, trigger));
  }

  private async destroyNow(sandboxId: string, trigger: SandboxTeardownReceipt['trigger']): Promise<SandboxTeardownReceipt> {
    const { ledger, kv, clock } = this.h;
    const row = ledger.getSandbox(sandboxId);
    if (!row) throw new EngineError('not_found', `sandbox ${sandboxId} not found`);
    if (row.status !== 'applied' && row.status !== 'failed') throw new EngineError('invalid_state', `sandbox ${sandboxId} is ${row.status}; there is nothing to end`);
    const active = this.activeRuns(sandboxId);
    if (active.length > 0 || this.h.hasPendingStarts()) {
      throw new EngineError('sandbox_in_use', `sandbox ${sandboxId} is in use by ${active.map((o) => o.id).join(', ') || 'a run that is starting'}`);
    }
    const region = row.region;
    const outputs = (row.outputs as unknown as SandboxOutputs | null) ?? (await this.h.provider.outputs(sandboxId).catch(() => null));
    const errors: string[] = [];
    this.phase(sandboxId, 'destroying', region, 'removing the environment');

    // 1. finish the cloud side of every ended run (the destroy refuses while their identities or instances remain)
    const runs = ledger.optimizationsInSandbox(sandboxId);
    for (const o of runs) {
      if (!ENDED.has(o.status)) continue;
      try {
        await this.h.teardown(o.id);
      } catch (err) {
        errors.push(`teardown ${o.id}: ${(err as Error).message}`);
      }
    }
    const reports = runs.map((o) => this.h.teardownReport(o.id)).filter((r): r is TeardownReport => r !== undefined);
    for (const r of reports) errors.push(...r.errors.filter((e) => !errors.includes(e)));

    // 2. destroy through the lifecycle (same orphan check as the explicit path)
    try {
      await this.h.lifecycleCall(() => this.h.lifecycle.destroy(sandboxId));
    } catch (err) {
      this.phase(sandboxId, 'failed', region, `could not remove the environment: ${(err as Error).message}`);
      throw err;
    }

    // 3. verify: nothing tagged is left running
    const orphans: SandboxTeardownReceipt['orphans'] = [];
    try {
      for (const i of await this.h.compute.listTagged()) if (i.state !== 'terminated') orphans.push({ id: i.id, role: i.role, state: i.state });
    } catch (err) {
      // No compute without sandbox outputs: nothing can be running. Anything else is reported, not hidden.
      if (!(err instanceof EngineError && err.code === 'not_configured')) errors.push(`verify listTagged: ${(err as Error).message}`);
    }

    const after = ledger.optimizationsInSandbox(sandboxId);
    const spendByCategory: Record<string, number> = {};
    let spend = 0;
    const runSummaries = after.map((o) => {
      const usd = ledger.totalSpend(o.id);
      spend += usd;
      for (const [cat, v] of Object.entries(ledger.spendBreakdown(o.id).byCategory)) if (v) spendByCategory[cat] = (spendByCategory[cat] ?? 0) + v;
      return { optimizationId: o.id, status: o.status, spendUsd: usd };
    });
    const instances = new Set<string>(Object.values(outputs?.environment?.machines ?? {}).map((m) => m.id).filter(Boolean));
    for (const o of after) {
      for (const r of this.h.ledger.sqlite.prepare("SELECT DISTINCT source FROM spend_events WHERE optimization_id = ? AND source LIKE 'ec2:%'").all(o.id) as Array<{ source: string }>) {
        instances.add(r.source.slice('ec2:'.length));
      }
    }
    for (const r of reports) for (const i of r.terminatedInstanceIds) instances.add(i);

    const receipt: SandboxTeardownReceipt = {
      sandboxId,
      region,
      trigger,
      createdAt: row.appliedAt,
      destroyedAt: clock.now(),
      runs: runSummaries,
      spendUsd: spend,
      spendByCategory,
      removed: {
        instances: [...instances],
        agentIdentities: [...new Set(reports.flatMap((r) => r.deletedIdentitiesFor))],
        secrets: [...new Set(reports.flatMap((r) => r.deletedSecretsFor))],
        resources: outputs ? resourcesOf(outputs) : [],
      },
      nothingRunning: orphans.length === 0 && !errors.some((e) => e.startsWith('verify listTagged')),
      orphans,
      errors,
      summary: '',
    };
    receipt.summary = summarize(receipt);
    kv.setJson(K.sandboxReceipt(sandboxId), receipt);
    for (const o of after) kv.setJson(K.optReceipt(o.id), receipt);
    kv.delete(K.sandboxAuto(sandboxId));
    kv.delete(K.sandboxIdle(sandboxId));
    this.retryAfter.delete(sandboxId);
    this.phase(sandboxId, 'destroyed', region, receipt.summary);
    return receipt;
  }

  // ------------------------------------------------------------------ reads

  receiptFor(optimizationId: string): SandboxTeardownReceipt | null {
    return this.h.kv.getJson<SandboxTeardownReceipt>(K.optReceipt(optimizationId)) ?? null;
  }

  async activity(sandboxId: string): Promise<SandboxActivity> {
    const { ledger, kv, settings } = this.h;
    const row = ledger.getSandbox(sandboxId);
    const auto = kv.has(K.sandboxAuto(sandboxId));
    const active = row?.status === 'applied' ? this.activeRuns(sandboxId) : [];
    let instancesRunning: number | null = null;
    if (row?.status === 'applied') {
      try {
        instancesRunning = (await this.h.compute.listTagged()).filter((i) => i.state !== 'terminated').length;
      } catch {
        instancesRunning = null;
      }
    } else instancesRunning = 0;
    const idle = row?.status === 'applied' && active.length === 0 ? this.idleSince(sandboxId) : null;
    return {
      sandboxId,
      status: row?.status ?? 'absent',
      region: row?.region ?? null,
      auto,
      activeRuns: active.map((o) => o.id),
      instancesRunning,
      spendUsd: active.reduce((sum, o) => sum + ledger.totalSpend(o.id), 0),
      idleSince: idle,
      destroyAt: idle !== null && auto ? idle + settings.sandbox.idleMs : null,
      lastReceipt: kv.getJson<SandboxTeardownReceipt>(K.sandboxReceipt(sandboxId)) ?? null,
    };
  }

  /** An explicit apply / destroy takes the sandbox out of automatic management. */
  release(sandboxId: string): void {
    this.h.kv.delete(K.sandboxAuto(sandboxId));
    this.h.kv.delete(K.sandboxIdle(sandboxId));
  }
}

function resourcesOf(o: SandboxOutputs): string[] {
  const env = o.environment;
  return [
    ...(env ? env.resources.map((r) => `environment (${env.scope}): ${r}`) : []),
    `S3 bucket ${o.artifactsBucket} (artifacts)`,
    `S3 bucket ${o.sealedBucket} (sealed holdout)`,
    `network subnet ${o.network.subnetId}`,
    `security groups ${o.network.agentRunnerSecurityGroupId}, ${o.network.benchSecurityGroupId}`,
    `agent permission boundary ${o.agentBoundaryRef}`,
    `evaluator identity ${o.evalIdentityRef}`,
  ];
}

function summarize(r: SandboxTeardownReceipt): string {
  const usd = `$${r.spendUsd.toFixed(2)}`;
  const runs = `${r.runs.length} run${r.runs.length === 1 ? '' : 's'}`;
  const left = r.nothingRunning ? 'Nothing is left running.' : `${r.orphans.length || 'Some'} resource(s) may still be running: check the environment.`;
  return `Environment in ${r.region} removed after ${runs}, ${usd} spent. ${r.removed.instances.length} instance(s) terminated, ${r.removed.resources.length} resources removed. ${left}`;
}
