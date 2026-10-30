import type { OptimizationRow, SandboxProviderKind, SandboxRow } from '../ledger/schema.js';
import type { Ledger } from '../ledger/store.js';
import type { SandboxInput, SandboxOutputs, SandboxPlan, SandboxProvider, SandboxState } from '../ports/index.js';

export type SandboxLifecycleErrorCode = 'invalid_state' | 'not_found' | 'sandbox_in_use' | 'orphans';

export class SandboxLifecycleError extends Error {
  constructor(
    readonly code: SandboxLifecycleErrorCode,
    message: string,
    readonly details?: unknown,
  ) {
    super(message);
    this.name = 'SandboxLifecycleError';
  }
}

/** Remembers the input a sandbox was applied with, so destroy runs with the same variables. */
export interface SandboxInputStore {
  get(sandboxId: string): SandboxInput | undefined;
  set(sandboxId: string, input: SandboxInput): void;
}

export interface SandboxLifecycleOptions {
  ledger: Ledger;
  provider: SandboxProvider;
  /** Ledger `sandboxes.provider`: `aws` for the tofu provider, `mock` for simulation. */
  kind: SandboxProviderKind;
  /**
   * Teardown guard beyond running optimizations: returns the reasons the sandbox must not be destroyed yet
   * (tagged instances still alive, agent identities not deleted). Fail closed: a throw blocks the destroy.
   */
  blockers?: (sandboxId: string, optimizations: OptimizationRow[]) => Promise<string[]> | string[];
  inputs?: SandboxInputStore;
}

const ACTIVE = new Set(['baselining', 'running', 'stopping']);
/** The provider refuses a destroy before touching anything (infra/README.md teardown order). */
const isTeardownBlocked = (err: unknown): boolean => err instanceof Error && err.name === 'TeardownBlockedError';

const ACCOUNT_IN_ARN = /^arn:aws[^:]*:[^:]*:[^:]*:(\d{12}):/;
const accountIdOf = (outputs: SandboxOutputs): string | null =>
  ACCOUNT_IN_ARN.exec(outputs.evalIdentityRef)?.[1] ?? ACCOUNT_IN_ARN.exec(outputs.agentBoundaryRef)?.[1] ?? null;

/**
 * Drives a sandbox through `planned -> applying -> applied -> destroying -> destroyed / failed`, recording each
 * transition (and the contract outputs on apply) in the ledger. Shared by the engine and the desktop dev stub so
 * both enforce the same rules:
 *
 * - apply is allowed from absent / planned / destroyed / failed / applied (a re-apply converges);
 * - destroy is refused while an optimization in the sandbox is baselining, running or stopping, and while the
 *   `blockers` hook reports orphans; a refusal changes nothing;
 * - a provider failure leaves the sandbox `failed` (its real state is unknown) and is rethrown;
 * - `recover()` marks an apply / destroy that a crash interrupted as `failed`.
 */
export class SandboxLifecycle {
  private readonly busy = new Set<string>();
  private readonly errors = new Map<string, string>();

  constructor(private readonly opts: SandboxLifecycleOptions) {}

  async plan(input: SandboxInput): Promise<SandboxPlan> {
    const plan = await this.opts.provider.plan(input);
    this.opts.ledger.recordSandbox({ id: input.sandboxId, provider: this.opts.kind, region: input.region });
    return plan;
  }

  async apply(input: SandboxInput): Promise<SandboxOutputs> {
    const { ledger, provider } = this.opts;
    this.claim(input.sandboxId, 'apply');
    try {
      ledger.recordSandbox({ id: input.sandboxId, provider: this.opts.kind, region: input.region });
      ledger.setSandboxStatus(input.sandboxId, 'applying');
      this.errors.delete(input.sandboxId);
      try {
        const outputs = await provider.apply(input);
        this.opts.inputs?.set(input.sandboxId, input);
        ledger.setSandboxStatus(input.sandboxId, 'applied', { outputs: outputs as unknown as Record<string, unknown>, accountId: accountIdOf(outputs) });
        return outputs;
      } catch (err) {
        this.errors.set(input.sandboxId, (err as Error).message);
        ledger.setSandboxStatus(input.sandboxId, 'failed');
        throw err;
      }
    } finally {
      this.busy.delete(input.sandboxId);
    }
  }

  async destroy(sandboxId: string): Promise<void> {
    const { ledger, provider } = this.opts;
    const row = ledger.getSandbox(sandboxId);
    if (!row) throw new SandboxLifecycleError('not_found', `sandbox ${sandboxId} not found`);
    if (row.status === 'destroyed') return;
    if (row.status === 'planned') throw new SandboxLifecycleError('invalid_state', `sandbox ${sandboxId} was never applied; there is nothing to destroy`);
    if (row.status === 'applying' || row.status === 'destroying') throw new SandboxLifecycleError('invalid_state', `sandbox ${sandboxId} is ${row.status}`);
    this.claim(sandboxId, 'destroy');
    try {
      const optimizations = ledger.optimizationsInSandbox(sandboxId);
      const active = optimizations.filter((o) => ACTIVE.has(o.status));
      if (active.length > 0) {
        throw new SandboxLifecycleError(
          'sandbox_in_use',
          `cannot destroy sandbox ${sandboxId}: ${active.length} optimization(s) still running (${active.map((o) => o.id).join(', ')}). Stop them first.`,
          active.map((o) => o.id),
        );
      }
      const blockers = await this.opts.blockers?.(sandboxId, optimizations);
      if (blockers && blockers.length > 0) {
        throw new SandboxLifecycleError('orphans', `cannot destroy sandbox ${sandboxId}: ${blockers.join('; ')}`, blockers);
      }
      const input = this.opts.inputs?.get(sandboxId) ?? fallbackInput(row);
      const before = row.status;
      ledger.setSandboxStatus(sandboxId, 'destroying');
      this.errors.delete(sandboxId);
      try {
        await provider.destroy(input);
      } catch (err) {
        if (isTeardownBlocked(err)) {
          // The provider refused before running `tofu destroy`: nothing changed, so the sandbox is as it was.
          ledger.setSandboxStatus(sandboxId, before);
          throw new SandboxLifecycleError('orphans', (err as Error).message);
        }
        this.errors.set(sandboxId, (err as Error).message);
        ledger.setSandboxStatus(sandboxId, 'failed');
        throw err;
      }
      ledger.setSandboxStatus(sandboxId, 'destroyed', { outputs: null });
    } finally {
      this.busy.delete(sandboxId);
    }
  }

  state(sandboxId: string): SandboxState {
    const row = this.opts.ledger.getSandbox(sandboxId);
    const optimizations = this.opts.ledger.optimizationsInSandbox(sandboxId).map((o) => ({ id: o.id, status: o.status }));
    if (!row) {
      return { sandboxId, status: 'absent', provider: null, region: null, accountId: null, outputs: null, appliedAt: null, destroyedAt: null, lastError: null, optimizations };
    }
    return {
      sandboxId,
      status: row.status,
      provider: row.provider,
      region: row.region,
      accountId: row.accountId,
      outputs: (row.outputs as unknown as SandboxOutputs | null) ?? null,
      appliedAt: row.appliedAt,
      destroyedAt: row.destroyedAt,
      lastError: this.errors.get(sandboxId) ?? null,
      optimizations,
    };
  }

  /** An apply / destroy cut short by a crash leaves unknown infrastructure behind: mark it failed so it can be retried. */
  recover(): string[] {
    const fixed: string[] = [];
    for (const row of this.opts.ledger.listSandboxes()) {
      if (row.status === 'applying' || row.status === 'destroying') {
        this.opts.ledger.setSandboxStatus(row.id, 'failed');
        this.errors.set(row.id, `${row.status === 'applying' ? 'apply' : 'destroy'} was interrupted; run it again to converge`);
        fixed.push(row.id);
      }
    }
    return fixed;
  }

  private claim(sandboxId: string, what: string): void {
    if (this.busy.has(sandboxId)) throw new SandboxLifecycleError('invalid_state', `sandbox ${sandboxId} is busy (another ${what} is running)`);
    this.busy.add(sandboxId);
  }
}

function fallbackInput(row: SandboxRow): SandboxInput {
  return { sandboxId: row.id, region: row.region, allowedInstanceTypes: ['c7i.2xlarge'], maxInstancesPerAgent: 2, maxInstanceLifetimeHours: 4 };
}
