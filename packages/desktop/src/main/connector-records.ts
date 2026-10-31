import type { ConnectorAgent, ConnectorKind, RepoConnections } from '../shared/connectors.js';

/**
 * What is persisted about connectors (non-secret metadata, in the settings file). Secrets (GitHub tokens, API keys) are never
 * part of a record: they sit encrypted in the same file under `connectorSecrets[id]`, written only through the OS keychain.
 */
interface Common {
  id: string;
  name: string;
  createdAt: number;
}

export interface GithubRecord extends Common {
  kind: 'github';
  login: string;
  source: 'device' | 'token';
}

export interface AwsRecord extends Common {
  kind: 'aws';
  profile: string;
  region: string;
  monthlyCapUsd: number | null;
}

export interface AgentRecord extends Common {
  kind: 'agent';
  agent: ConnectorAgent;
  source: 'login' | 'api-key';
  /** `····4f2a` for a stored key (non-secret). */
  keyHint: string | null;
}

export type ConnectorRecord = GithubRecord | AwsRecord | AgentRecord;

export interface ConnectorsPersisted {
  /** The single GitHub token / AWS profile / API keys of the first versions were turned into connectors. */
  migrated: boolean;
  items: ConnectorRecord[];
  /** One default connector id per kind. */
  defaults: Partial<Record<ConnectorKind, string>>;
  /** Detected CLI logins the user removed (not added again automatically). */
  dismissed: string[];
  /** Remembered choice per repo (key: lower-cased `owner/name`). */
  selections: Record<string, RepoConnections>;
}

export const emptyConnectors = (): ConnectorsPersisted => ({ migrated: false, items: [], defaults: {}, dismissed: [], selections: {} });
