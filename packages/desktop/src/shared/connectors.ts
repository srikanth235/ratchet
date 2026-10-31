// Connectors: the accounts an optimization can use, several of each kind, stored once and picked per run.
// Types only (no zod, no Node): shared by the main process, the preload script and the renderer. Secrets (GitHub tokens, API
// keys) are never part of any of these; they live in the OS keychain and only the main process reads them.
import type { GithubFlowVM } from './types.js';

export type ConnectorKind = 'github' | 'aws' | 'agent';
/** The two agent CLIs Ratchet drives. */
export type ConnectorAgent = 'claude-code' | 'codex';
/** Where an agent runs: on this Mac (your CLI login or a key) or on cloud runners (an API key only). */
export type ConnectorWhere = 'local' | 'cloud';

/**
 * - `connected`  a GitHub account / AWS profile / agent login that works
 * - `expired`    it worked once and needs signing in again (GitHub token revoked, `aws sso login` needed)
 * - `signed-out` nothing to use: an agent CLI that is not signed in or not installed, a GitHub account without a token
 * - `valid`      an API key the provider accepted
 * - `invalid`    an API key (or static AWS keys) the provider rejected
 */
export type ConnectorStatus = 'connected' | 'expired' | 'signed-out' | 'valid' | 'invalid';

export type GithubConnectorSource = 'device' | 'token' | 'env';
export type AgentConnectorSource = 'login' | 'api-key';
export type AwsAuthStyle = 'sso' | 'role' | 'keys' | 'process' | 'other';

interface ConnectorBase {
  id: string;
  kind: ConnectorKind;
  /** What the user sees and can rename ("srikanth235", "Claude Code · team key"). */
  name: string;
  /** One muted line under the name. Backticks mark a command (`codex login`) to show in monospace. */
  detail: string;
  status: ConnectorStatus;
  /** The default of its kind (one per kind; for agents, the one used when a run does not choose). */
  isDefault: boolean;
}

export interface GithubConnectorVM extends ConnectorBase {
  kind: 'github';
  /** The account's login, null until it is known (a token from the environment that was not looked up yet). */
  login: string | null;
  source: GithubConnectorSource;
}

export interface AwsConnectorVM extends ConnectorBase {
  kind: 'aws';
  /** Profile name in ~/.aws/config (names only; credentials stay where AWS keeps them). */
  profile: string;
  region: string;
  auth: AwsAuthStyle;
  /** Optional monthly spend cap in USD. */
  monthlyCapUsd: number | null;
}

export interface AgentConnectorVM extends ConnectorBase {
  kind: 'agent';
  agent: ConnectorAgent;
  source: AgentConnectorSource;
  /** `login` runs on this Mac only; `api-key` can run here or on cloud runners. */
  runsOn: ConnectorWhere[];
  /** Detected CLI version (login) or null. */
  version: string | null;
  /** Last characters of a stored key, e.g. `····4f2a`. Never the key. */
  keyHint: string | null;
}

export type ConnectorVM = GithubConnectorVM | AwsConnectorVM | AgentConnectorVM;

/** Everything the Connectors screen shows. */
export interface ConnectorsVM {
  connectors: ConnectorVM[];
  /** The GitHub device flow in progress (or just ended), for the code to show. */
  githubFlow: GithubFlowVM | null;
  /** False when the OS keychain is unavailable: tokens and keys cannot be stored (the environment is still honoured). */
  encryptionAvailable: boolean;
  /** True when a real OAuth client id is configured for "Sign in with GitHub". */
  clientIdConfigured: boolean;
}

// ------------------------------------------------------------------ inputs

export interface AddGithubConnectorInput {
  /** A personal access token or an app installation token; checked against GitHub before it is kept. */
  token: string;
  name?: string;
}

export interface StartGithubSignInInput {
  /** Re-sign-in: store the new token on this connector (default: a new one, or the one with the same login). */
  connectorId?: string;
}

export interface AddAwsConnectorInput {
  /** A profile from ~/.aws/config or ~/.aws/credentials. */
  profile: string;
  /** Default us-east-1. */
  region?: string;
  /** USD per month; omit or null for no cap. */
  monthlyCapUsd?: number | null;
  name?: string;
}

export type AddAgentConnectorInput =
  /** Adopt a detected CLI login (one that was removed earlier). */
  | { agent: ConnectorAgent; source: 'login' }
  | { agent: ConnectorAgent; source: 'api-key'; apiKey: string; name?: string };

export interface UpdateConnectorInput {
  name?: string;
  /** AWS only. */
  region?: string;
  /** AWS only; null removes the cap. */
  monthlyCapUsd?: number | null;
}

/** Result of "Test connection" on one connector. */
export interface ConnectorTestVM {
  ok: boolean;
  detail: string;
  /** The connector with its fresh status (null if it no longer exists). */
  connector: ConnectorVM | null;
}

// ------------------------------------------------------------------ per-repo remembered selection

export interface AgentSelection {
  connectorId: string;
  where: ConnectorWhere;
}

/** What one repo's runs use: one GitHub account, one AWS profile, and one connector per agent with where it runs. */
export interface RepoConnections {
  github: string;
  aws: string;
  agents: AgentSelection[];
}

export interface RepoConnectionsVM {
  /** `owner/name`, lower-cased: the key the selection is remembered under. */
  repo: string;
  /** What was saved for this repo last time (only entries that still exist); null if nothing usable was saved. */
  saved: RepoConnections | null;
  /** What a run would use right now: the saved choice where there is one, else the defaults. `null` = nothing connected of that kind. */
  effective: { github: string | null; aws: string | null; agents: AgentSelection[] };
}

/** Normalised key of a repo (`Owner/Name` and `owner/name` are the same repo). */
export const repoKey = (repo: string): string => repo.trim().toLowerCase();
