/**
 * Dynamic environments (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"): the environment an optimization is measured in is
 * sized to the task by a setup agent BEFORE the run, validated against the sandbox contract, costed, and locked once the user
 * approves it. These are the data types of that flow; `EnvironmentDrafter` (below) is the service the desktop calls.
 */
import type { EnvironmentScope } from '../config/schema.js';
import type { AgentKind } from '../ledger/schema.js';

export type { EnvironmentScope };

/** What a machine of the environment does. Mirrors the `role` of the base module's `machines` map. */
export const ENVIRONMENT_ROLES = ['app', 'database', 'cache', 'load', 'evaluator', 'other'] as const;
export type EnvironmentRole = (typeof ENVIRONMENT_ROLES)[number];

/** One row of "What gets created". */
export interface EnvironmentResource {
  /** "API", "Postgres 16", "Load generator". */
  name: string;
  role: EnvironmentRole;
  /** Instance type: `c7i.2xlarge`. */
  machine: string;
  /** One plain line: "shop-api in the repo's container". */
  detail: string;
  usdPerHour: number;
}

/** A file of the draft, repo-relative POSIX path (`ratchet.yaml`, or under `infra/ratchet/`). */
export interface EnvironmentFile {
  path: string;
  content: string;
}

/** One of Ratchet's checks on the draft ("isolated", "results sealed", "same hardware every time", "full teardown", ...). */
export interface EnvironmentCheck {
  id: string;
  /** Plain words for the UI. */
  label: string;
  ok: boolean;
  /** Why it passed or failed, one line (the list of offending lines when it failed). */
  detail: string;
}

export interface EnvironmentCost {
  /** Sum of the resources' hourly prices (excludes the sandbox's fixed private-network cost, ~$0.04/h, every run pays). */
  usdPerHour: number;
  /** Minutes until the environment is ready to measure (sandbox boot + one per machine). */
  startupMinutes: number;
  /** `usdPerHour x (run hours + start-up)` for the run's time limit. */
  estimateForRunUsd: number;
}

/** What the setup agent proposed, as the "New optimization > Environment" step shows it. */
export interface EnvironmentDraft {
  scope: EnvironmentScope;
  /** Why this scope, in a sentence or two ("p95 depends on database queries, so a single machine wouldn't measure it"). */
  reason: string;
  resources: EnvironmentResource[];
  /** What would be committed: `ratchet.yaml` plus the files under `infra/ratchet/`. */
  files: EnvironmentFile[];
  checks: EnvironmentCheck[];
  cost: EnvironmentCost;
  /** Which connector drafted it: `claude-code`, `codex` (or `mock`). */
  agent: AgentKind;
  /** How long drafting took. */
  durationS: number;
  /** What the setup agent cost in model tokens. */
  usdSpent: number;
}

/** True when every check passed: the draft may be approved. */
export function draftPassed(draft: Pick<EnvironmentDraft, 'checks'>): boolean {
  return draft.checks.length > 0 && draft.checks.every((c) => c.ok);
}

// ---------------------------------------------------------------- the drafter service

export interface DraftEnvironmentInput {
  /** Repository URL the optimization runs on (what `EngineCreateOptimizationInput.repoUrl` takes). */
  repo: string;
  /** The protected ref the files are committed to / the PR targets. Default `main`. */
  baseRef?: string;
  /** What the user wants optimized, in their words ("make checkout p95 faster"). */
  goal: string;
  /** The user's agent connector that does the drafting (their own login, run locally). */
  agentConnector: AgentKind;
  /** The AWS profile selected in Settings: `tofu plan` is checked against that account. */
  awsProfile?: string;
  /** Region of the sandbox (default: `environment.region` of ratchet.yaml, else `us-east-1`). */
  region?: string;
  /** The run's time limit, for the cost estimate (default 2 h). */
  timeLimitSeconds?: number;
  /** Path of the config inside the repo (default `ratchet.yaml`). */
  configPath?: string;
  /** Progress while the agent works (for a spinner / live log). */
  onProgress?: (e: EnvironmentDraftProgress) => void;
}

export interface EnvironmentDraftProgress {
  draftId: string;
  phase: 'reading' | 'drafting' | 'validating' | 'done' | 'failed';
  text: string;
}

export type EnvironmentDraftStatus = 'drafting' | 'ready' | 'failed' | 'approved';

/** One turn of the revise loop. */
export interface EnvironmentDraftMessage {
  from: 'user' | 'agent';
  text: string;
  at: number;
}

/** A draft as the service keeps it: the draft itself plus where it stands in the draft / revise / approve loop. */
export interface EnvironmentDraftRecord {
  id: string;
  repo: string;
  baseRef: string;
  goal: string;
  awsProfile?: string;
  region: string;
  status: EnvironmentDraftStatus;
  /** 1 for the first draft, +1 for every `revise`. */
  revision: number;
  /** Set unless `status` is `failed` before the first draft. */
  draft: EnvironmentDraft | null;
  /** The user's change requests and the agent's replies. */
  messages: EnvironmentDraftMessage[];
  /** Why drafting failed. */
  error?: string;
  /** Set once approved. */
  approval?: EnvironmentApproval;
  createdAt: number;
  updatedAt: number;
}

export type EnvironmentApprovalMode = 'commit' | 'pr';

export interface ApproveEnvironmentOptions {
  /** `commit`: straight onto `baseRef`. `pr`: a branch and a pull request the user merges. Default `pr`. */
  mode?: EnvironmentApprovalMode;
}

export interface EnvironmentApproval {
  mode: EnvironmentApprovalMode;
  /** The commit holding the files (on `baseRef` for `commit`, on `branch` for `pr`). */
  sha: string;
  /** `pr` only. */
  branch?: string;
  /** `pr` only. */
  prUrl?: string;
  /** Repo paths written. */
  files: string[];
  /** The environment is locked from here (for `pr`: once the PR is merged): agents' candidates touching it are blocked. */
  locked: true;
  at: number;
}

/**
 * Drafts, revises and approves an optimization's environment. Real implementation: a setup agent (the user's Claude Code /
 * Codex, local) writes the files, Ratchet validates and costs them; mock: deterministic drafts per scope for simulation, demos
 * and tests. Implementations never apply anything: applying is the engine's job, at run start, from the protected ref.
 */
export interface EnvironmentDrafter {
  draft(input: DraftEnvironmentInput): Promise<EnvironmentDraftRecord>;
  /** Ask the same agent for a change ("use a smaller setup", "use RDS instead of a Postgres container"). Re-validates and re-costs. */
  revise(draftId: string, message: string): Promise<EnvironmentDraftRecord>;
  /** Commit the files to the repo's protected ref (directly or as a PR). Refuses a draft whose checks did not all pass. */
  approve(draftId: string, options?: ApproveEnvironmentOptions): Promise<EnvironmentDraftRecord>;
  get(draftId: string): EnvironmentDraftRecord | undefined;
  list(): EnvironmentDraftRecord[];
  /** Forget a draft that was not approved ("Back"). */
  discard(draftId: string): void;
}

/**
 * Runs `tofu validate` and `tofu plan` on a drafted module in the user's AWS account (the "plans cleanly in your account" check).
 * Real implementation: `TofuEnvironmentChecker` in @ratchet/aws. Without one, a draft with OpenTofu cannot pass the checks:
 * Ratchet does not claim a plan it did not run.
 */
export interface EnvironmentChecker {
  plan(input: {
    scope: EnvironmentScope;
    /** Repo paths under `infra/ratchet/`. */
    files: EnvironmentFile[];
    region: string;
    awsProfile?: string | undefined;
  }): Promise<{ ok: boolean; detail: string }>;
}
