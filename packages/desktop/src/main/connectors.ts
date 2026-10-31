import { randomBytes } from 'node:crypto';
import type { HarnessInfo } from '@ratchet/agents';
import {
  repoKey,
  type AddAgentConnectorInput,
  type AddAwsConnectorInput,
  type AgentConnectorVM,
  type AgentSelection,
  type AwsAuthStyle,
  type AwsConnectorVM,
  type ConnectorAgent,
  type ConnectorKind,
  type ConnectorStatus,
  type ConnectorVM,
  type ConnectorWhere,
  type GithubConnectorVM,
  type RepoConnections,
  type RepoConnectionsVM,
  type UpdateConnectorInput,
} from '../shared/connectors.js';
import { DEFAULT_AWS_REGION, validateLiveSetting, validateSecretFormat } from '../shared/live-settings.js';
import type { SecretName } from '../shared/types.js';
import type { AwsProfileList } from './aws-profiles.js';
import type { AgentRecord, AwsRecord, ConnectorRecord, GithubRecord } from './connector-records.js';
import type { GithubApi } from './github.js';
import type { HarnessCache } from './harnesses.js';
import type { SettingsStore } from './settings.js';

/** The slice of @ratchet/runtime's ConnectionTester the connectors use (structural, so renderer tests need no AWS SDK). */
export interface ConnectionTesterLike {
  aws(i: { region: string; profile?: string | undefined; expectedAccountId?: string | undefined }): Promise<{ ok: boolean; detail: string }>;
  anthropic(i: { apiKey: string }): Promise<{ ok: boolean; detail: string }>;
  openai(i: { apiKey: string }): Promise<{ ok: boolean; detail: string }>;
}

export interface ConnectorDeps {
  github?: GithubApi | undefined;
  /** Detected agent CLIs ("your login" connectors). Without it none are found. */
  harnesses?: HarnessCache | undefined;
  /** The profiles in ~/.aws (names only). */
  listAwsProfiles?: (() => AwsProfileList) | undefined;
  /** Probes for AWS profiles and API keys. Without it those connectors keep the status they were given. */
  tester?: ConnectionTesterLike | undefined;
  now?: () => number;
}

/** What a run uses, with the secrets in it: main process only, never serialised to the renderer. */
export interface ResolvedAgent {
  connectorId: string;
  name: string;
  agent: ConnectorAgent;
  source: 'login' | 'api-key';
  where: ConnectorWhere;
  /** The stored key of an `api-key` connector (null for a CLI login). */
  apiKey: string | null;
}

export interface ResolvedConnections {
  github: { id: string; name: string; login: string | null; token: string } | null;
  aws: { id: string; name: string; profile: string; region: string; monthlyCapUsd: number | null } | null;
  /** The chosen agents (the remembered selection for the repo, else the default connector of each agent). */
  agents: ResolvedAgent[];
  /** The key a cloud runner of each agent gets (only from a chosen or default `api-key` connector; never from a login). */
  cloudKeys: Partial<Record<ConnectorAgent, string>>;
}

export class ConnectorError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConnectorError';
  }
}

export const GITHUB_ENV_ID = 'github-env';
const AGENT_LABEL: Record<ConnectorAgent, string> = { 'claude-code': 'Claude Code', codex: 'Codex' };
const AGENTS: ConnectorAgent[] = ['claude-code', 'codex'];
const SECRET_FOR: Record<ConnectorAgent, SecretName> = { 'claude-code': 'anthropicApiKey', codex: 'openaiApiKey' };
const LOGIN_COMMAND: Record<ConnectorAgent, string> = { 'claude-code': '`claude`', codex: '`codex login`' };
const MAX_CAP_USD = 10_000_000;

/** A probe that failed because the network or the service was out of reach says nothing about the account. */
const unreachable = (detail: string): boolean => /ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ECONNRESET|ETIMEDOUT|getaddrinfo|fetch failed|network|timed out|timeout|could not connect|could not reach|answered 5\d\d/i.test(detail);

const newId = (prefix: string): string => `${prefix}-${randomBytes(4).toString('hex')}`;
const loginId = (agent: ConnectorAgent): string => `agent-${agent}-login`;
const scrub = (text: string, secrets: Array<string | null>): string => secrets.reduce<string>((t, s) => (s && s.length >= 4 ? t.split(s).join('***') : t), text);

/**
 * Saved connectors: several GitHub accounts, AWS profiles and agent credentials, one default per kind, a remembered choice per repo.
 * Metadata is kept in the settings file; tokens and keys go to the OS keychain (SettingsStore). Secrets only ever leave through
 * `resolve` / `githubToken` / `secretValue`, which the main process hands to the engine.
 */
export class ConnectorStore {
  private readonly statuses = new Map<string, ConnectorStatus>();
  private harness: HarnessInfo[] = [];

  constructor(
    private readonly settings: SettingsStore,
    private readonly deps: ConnectorDeps = {},
  ) {
    this.migrate();
  }

  private get data() {
    return this.settings.connectorData;
  }
  private now(): number {
    return (this.deps.now ?? Date.now)();
  }

  // ------------------------------------------------------------------ migration

  /** The single GitHub token, AWS profile and API keys of the first versions become connectors (once). */
  private migrate(): void {
    const d = this.data;
    if (d.migrated) return;
    const old = this.settings.legacy();
    if (old.locked) return; // a secret we cannot decrypt right now: try again on the next launch rather than lose it
    const t = this.now();
    const token = old.secrets.githubToken;
    if (token) {
      const id = newId('github');
      this.settings.setConnectorSecret(id, token);
      const login = old.githubLogin;
      d.items.push({ id, kind: 'github', name: login || 'GitHub', login, source: old.githubSource === 'device' ? 'device' : 'token', createdAt: t });
      d.defaults.github = id;
    }
    if (old.awsProfile) {
      const id = newId('aws');
      d.items.push({ id, kind: 'aws', name: old.awsProfile, profile: old.awsProfile, region: this.settings.live.awsRegion || DEFAULT_AWS_REGION, monthlyCapUsd: null, createdAt: t });
      d.defaults.aws = id;
    }
    for (const agent of AGENTS) {
      const key = old.secrets[SECRET_FOR[agent]];
      if (!key) continue;
      const id = newId(`agent-${agent}`);
      this.settings.setConnectorSecret(id, key);
      d.items.push({ id, kind: 'agent', name: `${AGENT_LABEL[agent]} · API key`, agent, source: 'api-key', keyHint: `····${key.slice(-4)}`, createdAt: t });
    }
    d.migrated = true;
    this.settings.clearLegacy(); // also saves
  }

  // ------------------------------------------------------------------ records

  /** Stored connectors, plus the GitHub token from the environment when there is one (not persisted, not removable). */
  private records(): ConnectorRecord[] {
    const out: ConnectorRecord[] = [...this.data.items];
    if (this.settings.envSecret('githubToken')) out.push({ id: GITHUB_ENV_ID, kind: 'github', name: 'GitHub (environment)', login: '', source: 'device', createdAt: Number.MAX_SAFE_INTEGER });
    return out;
  }
  private find(id: string): ConnectorRecord | undefined {
    return this.records().find((r) => r.id === id);
  }
  private need(id: string, kind?: ConnectorKind): ConnectorRecord {
    const r = this.find(id);
    if (!r || (kind && r.kind !== kind)) throw new ConnectorError('That connector no longer exists.');
    return r;
  }
  private isEnv(r: ConnectorRecord): boolean {
    return r.id === GITHUB_ENV_ID;
  }

  /** Detected CLI logins become connectors the first time they are seen (unless the user removed them). */
  private async sync(): Promise<void> {
    if (!this.deps.harnesses) return;
    this.harness = (await this.deps.harnesses.get()).harnesses;
    this.addDetectedLogins();
  }

  private addDetectedLogins(): void {
    let changed = false;
    for (const h of this.harness) {
      if (!h.installed || (h.kind !== 'claude-code' && h.kind !== 'codex')) continue;
      const id = loginId(h.kind);
      if (this.data.items.some((r) => r.id === id) || this.data.dismissed.includes(id)) continue;
      this.data.items.push({ id, kind: 'agent', name: `${AGENT_LABEL[h.kind]} · your login`, agent: h.kind, source: 'login', keyHint: null, createdAt: this.now() });
      changed = true;
    }
    if (changed) this.settings.saveConnectors();
  }

  private harnessOf(agent: ConnectorAgent): HarnessInfo | undefined {
    return this.harness.find((h) => h.kind === agent);
  }
  private awsList(): AwsProfileList | null {
    try {
      return this.deps.listAwsProfiles?.() ?? null;
    } catch {
      return null;
    }
  }
  /** Secret of a stored connector, or the environment's token for the environment connector. */
  private secretOf(r: ConnectorRecord): string | null {
    return this.isEnv(r) ? this.settings.envSecret('githubToken') : this.settings.getConnectorSecret(r.id);
  }

  // ------------------------------------------------------------------ status

  private statusOf(r: ConnectorRecord): ConnectorStatus {
    const probed = this.statuses.get(r.id);
    switch (r.kind) {
      case 'github':
        return this.secretOf(r) ? (probed ?? 'connected') : 'signed-out';
      case 'aws': {
        if (probed) return probed;
        const l = this.awsList();
        return l?.filesFound && !l.profiles.some((p) => p.name === r.profile) ? 'invalid' : 'connected';
      }
      case 'agent': {
        if (r.source === 'api-key') return this.secretOf(r) ? (probed ?? 'valid') : 'invalid';
        const h = this.harnessOf(r.agent);
        return h?.installed && h.auth !== 'signed-out' ? 'connected' : 'signed-out';
      }
    }
  }

  // ------------------------------------------------------------------ defaults

  /** The stored default if it still exists; else the first connector (for agents: the first usable one, logins first). */
  private defaultId(kind: ConnectorKind): string | null {
    const recs = this.records().filter((r) => r.kind === kind);
    const stored = this.data.defaults[kind];
    if (stored && recs.some((r) => r.id === stored)) return stored;
    if (kind !== 'agent') return recs[0]?.id ?? null;
    const usable = recs.filter((r) => ['connected', 'valid'].includes(this.statusOf(r)));
    return this.sortAgents(usable)[0]?.id ?? this.sortAgents(recs)[0]?.id ?? null;
  }

  private sortAgents<T extends ConnectorRecord>(list: T[]): T[] {
    const rank = (r: ConnectorRecord): number => (r.kind === 'agent' ? AGENTS.indexOf(r.agent) * 2 + (r.source === 'login' ? 0 : 1) : 0);
    return [...list].sort((a, b) => rank(a) - rank(b));
  }

  setDefault(id: string): void {
    const r = this.need(id);
    this.data.defaults[r.kind] = id;
    this.settings.saveConnectors();
  }

  /** The default connector of a kind, or null. */
  defaultOf(kind: ConnectorKind): ConnectorRecord | null {
    const id = this.defaultId(kind);
    return id ? (this.find(id) ?? null) : null;
  }

  // ------------------------------------------------------------------ view models

  private vm(r: ConnectorRecord, defaults: Partial<Record<ConnectorKind, string | null>>): ConnectorVM {
    const base = { id: r.id, name: r.name, status: this.statusOf(r), isDefault: defaults[r.kind] === r.id };
    switch (r.kind) {
      case 'github': {
        const source = this.isEnv(r) ? 'env' : r.source;
        const detail = source === 'env' ? 'From GITHUB_TOKEN in the environment' : source === 'device' ? 'Signed in with GitHub' : 'Access token';
        const status = base.status;
        const vm: GithubConnectorVM = {
          ...base,
          kind: 'github',
          detail: status === 'expired' ? `${detail} · sign in again` : status === 'signed-out' ? 'No token stored · sign in' : detail,
          login: r.login || null,
          source,
        };
        return vm;
      }
      case 'aws': {
        const auth = this.awsAuth(r.profile);
        const label = auth === 'sso' ? 'SSO' : auth === 'role' ? 'Assume role' : auth === 'process' ? 'Credential process' : '~/.aws profile';
        const cap = r.monthlyCapUsd !== null ? ` · cap $${r.monthlyCapUsd.toLocaleString('en-US')} / month` : '';
        const missing = base.status === 'invalid' ? ' · profile not found in ~/.aws' : '';
        const vm: AwsConnectorVM = { ...base, kind: 'aws', detail: `${label} · ${r.region}${cap}${missing}`, profile: r.profile, region: r.region, auth, monthlyCapUsd: r.monthlyCapUsd };
        return vm;
      }
      case 'agent': {
        const h = this.harnessOf(r.agent);
        const vm: AgentConnectorVM = { ...base, kind: 'agent', detail: '', agent: r.agent, source: r.source, runsOn: r.source === 'login' ? ['local'] : ['local', 'cloud'], version: null, keyHint: r.keyHint };
        if (r.source === 'api-key') {
          vm.detail = `API key ${r.keyHint ?? ''} · also runs in the cloud`.replace('  ', ' ');
          if (base.status === 'invalid') vm.detail = `API key ${r.keyHint ?? ''} · the provider rejected it`.replace('  ', ' ');
        } else if (!h?.installed) {
          vm.detail = 'Not found on this Mac';
        } else {
          vm.version = h.version;
          vm.detail = `Found on this Mac${h.version ? ` · v${h.version}` : ''}${h.auth === 'signed-out' ? ` · run ${LOGIN_COMMAND[r.agent]}` : ''}`;
        }
        return vm;
      }
    }
  }

  private awsAuth(profile: string): AwsAuthStyle {
    return this.awsList()?.profiles.find((p) => p.name === profile)?.kind ?? 'other';
  }

  private defaultsNow(): Record<ConnectorKind, string | null> {
    return { github: this.defaultId('github'), aws: this.defaultId('aws'), agent: this.defaultId('agent') };
  }

  /** Every connector as the Connectors screen shows it (detected CLI logins included). */
  async list(): Promise<ConnectorVM[]> {
    await this.sync();
    return this.snapshot();
  }

  /** Same without probing the machine again. */
  snapshot(): ConnectorVM[] {
    const defaults = this.defaultsNow();
    const kindRank: Record<ConnectorKind, number> = { github: 0, aws: 1, agent: 2 };
    const sorted = [...this.records()].sort((a, b) => kindRank[a.kind] - kindRank[b.kind]);
    const agents = this.sortAgents(sorted.filter((r) => r.kind === 'agent'));
    const rest = sorted.filter((r) => r.kind !== 'agent');
    return [...rest, ...agents].map((r) => this.vm(r, defaults));
  }

  vmOf(id: string): ConnectorVM | null {
    const r = this.find(id);
    return r ? this.vm(r, this.defaultsNow()) : null;
  }

  // ------------------------------------------------------------------ add / update / remove

  private nameTaken(kind: ConnectorKind, name: string, except?: string): boolean {
    return this.records().some((r) => r.kind === kind && r.id !== except && r.name.toLowerCase() === name.toLowerCase());
  }
  private uniqueName(kind: ConnectorKind, wanted: string): string {
    let name = wanted;
    for (let n = 2; this.nameTaken(kind, name); n++) name = `${wanted} (${n})`;
    return name;
  }
  private cleanName(name: string): string {
    const n = name.trim();
    if (n === '' || n.length > 80) throw new ConnectorError('Use a name of 1 to 80 characters.');
    return n;
  }
  /** The first connector of a kind becomes its default. */
  private defaultIfFirst(kind: ConnectorKind, id: string): void {
    if (!this.data.defaults[kind] || !this.data.items.some((r) => r.id === this.data.defaults[kind])) this.data.defaults[kind] = id;
  }

  /**
   * Store a GitHub token as a connector. The same account (login) signing in again replaces its token; `connectorId` re-signs-in
   * a specific connector (and follows the login if it changed).
   */
  upsertGithub(input: { login: string; token: string; source: 'device' | 'token'; connectorId?: string; name?: string; makeDefault?: boolean }): GithubRecord {
    const { login, token, source } = input;
    let target = input.connectorId ? this.data.items.find((r): r is GithubRecord => r.id === input.connectorId && r.kind === 'github') : undefined;
    target ??= this.data.items.find((r): r is GithubRecord => r.kind === 'github' && r.login !== '' && r.login.toLowerCase() === login.toLowerCase());
    if (input.connectorId && !target) throw new ConnectorError('That connector no longer exists.');
    const id = target?.id ?? newId('github');
    this.settings.setConnectorSecret(id, token); // throws (changing nothing) when the keychain is unavailable
    let rec: GithubRecord;
    if (target) {
      if (input.name) target.name = this.cleanName(input.name);
      else if (target.name === target.login || target.name === 'GitHub') target.name = login;
      target.login = login;
      target.source = source;
      rec = target;
    } else {
      rec = { id, kind: 'github', name: this.uniqueName('github', input.name ? this.cleanName(input.name) : login), login, source, createdAt: this.now() };
      this.data.items.push(rec);
    }
    this.statuses.set(id, 'connected');
    this.defaultIfFirst('github', id);
    if (input.makeDefault) this.data.defaults.github = id;
    this.settings.saveConnectors();
    return rec;
  }

  /** A pasted token (personal access token or app token): proven against GitHub before it is kept. */
  async addGithubToken(token: string, name?: string, makeDefault = false): Promise<GithubRecord> {
    const problem = validateSecretFormat('githubToken', token);
    if (problem) throw new ConnectorError(problem);
    if (!this.deps.github) throw new ConnectorError('GitHub is not available in this build.');
    let login: string;
    try {
      login = (await this.deps.github.user(token)).login;
    } catch (err) {
      throw new ConnectorError(scrub((err as Error).message, [token]));
    }
    return this.upsertGithub({ login, token, source: 'token', ...(name ? { name } : {}), makeDefault });
  }

  private validRegion(region: string): string {
    const problem = validateLiveSetting('awsRegion', region);
    if (problem || region.trim() === '') throw new ConnectorError(problem ?? 'Choose a region such as us-east-1.');
    return region.trim();
  }
  private validCap(cap: number | null | undefined): number | null {
    if (cap === null || cap === undefined) return null;
    if (!Number.isFinite(cap) || cap <= 0 || cap > MAX_CAP_USD) throw new ConnectorError('The monthly cap must be a positive number of US dollars.');
    return Math.round(cap * 100) / 100;
  }

  addAws(input: AddAwsConnectorInput & { makeDefault?: boolean }): AwsRecord {
    const profile = input.profile.trim();
    const problem = validateLiveSetting('awsProfile', profile);
    if (problem || profile === '') throw new ConnectorError(problem ?? 'Choose an AWS profile.');
    if (this.data.items.some((r) => r.kind === 'aws' && r.profile === profile)) throw new ConnectorError(`${profile} is already connected.`);
    const rec: AwsRecord = {
      id: newId('aws'),
      kind: 'aws',
      name: this.uniqueName('aws', input.name ? this.cleanName(input.name) : profile),
      profile,
      region: this.validRegion(input.region || DEFAULT_AWS_REGION),
      monthlyCapUsd: this.validCap(input.monthlyCapUsd),
      createdAt: this.now(),
    };
    this.data.items.push(rec);
    this.defaultIfFirst('aws', rec.id);
    if (input.makeDefault) this.data.defaults.aws = rec.id;
    this.settings.saveConnectors();
    return rec;
  }

  /** An API key for Claude Code or Codex, kept in the keychain and checked with the provider (a rejected key is kept as `invalid`). */
  async addAgent(input: AddAgentConnectorInput, opts: { waitForCheck?: boolean } = {}): Promise<AgentRecord> {
    if (input.source === 'login') {
      const id = loginId(input.agent);
      await this.sync();
      const existing = this.data.items.find((r) => r.id === id);
      if (existing) return existing as AgentRecord;
      const h = this.harnessOf(input.agent);
      if (!h?.installed) throw new ConnectorError(`${AGENT_LABEL[input.agent]} was not found on this Mac.`);
      this.data.dismissed = this.data.dismissed.filter((x) => x !== id);
      this.addDetectedLogins();
      return this.data.items.find((r) => r.id === id) as AgentRecord;
    }
    const secretName = SECRET_FOR[input.agent];
    const problem = validateSecretFormat(secretName, input.apiKey);
    if (problem) throw new ConnectorError(problem);
    const id = newId(`agent-${input.agent}`);
    this.settings.setConnectorSecret(id, input.apiKey);
    const rec: AgentRecord = {
      id,
      kind: 'agent',
      name: this.uniqueName('agent', input.name ? this.cleanName(input.name) : `${AGENT_LABEL[input.agent]} · API key`),
      agent: input.agent,
      source: 'api-key',
      keyHint: `····${input.apiKey.slice(-4)}`,
      createdAt: this.now(),
    };
    this.data.items.push(rec);
    this.settings.saveConnectors();
    const check = this.test(id).catch(() => undefined);
    if (opts.waitForCheck ?? true) await check;
    return rec;
  }

  update(id: string, patch: UpdateConnectorInput): void {
    const r = this.need(id);
    if (this.isEnv(r)) throw new ConnectorError('This account comes from the environment; change it there.');
    if (patch.name !== undefined) {
      const name = this.cleanName(patch.name);
      if (this.nameTaken(r.kind, name, id)) throw new ConnectorError(`There is already a connector named ${name}.`);
      r.name = name;
    }
    if (patch.region !== undefined || patch.monthlyCapUsd !== undefined) {
      if (r.kind !== 'aws') throw new ConnectorError('Only AWS connectors have a region and a spend cap.');
      if (patch.region !== undefined) r.region = this.validRegion(patch.region);
      if (patch.monthlyCapUsd !== undefined) r.monthlyCapUsd = this.validCap(patch.monthlyCapUsd);
    }
    this.settings.saveConnectors();
  }

  remove(id: string): void {
    const r = this.need(id);
    if (this.isEnv(r)) throw new ConnectorError('This account comes from GITHUB_TOKEN in the environment; unset it there.');
    const d = this.data;
    d.items = d.items.filter((x) => x.id !== id);
    if (d.defaults[r.kind] === id) delete d.defaults[r.kind];
    if (r.kind === 'agent' && r.source === 'login') d.dismissed = [...new Set([...d.dismissed, id])];
    this.settings.clearConnectorSecret(id);
    this.statuses.delete(id);
    this.settings.saveConnectors();
  }

  // ------------------------------------------------------------------ probes

  /** Check one connector against its provider and remember the status. A network failure leaves the status alone. */
  async test(id: string): Promise<{ ok: boolean; detail: string }> {
    const r = this.need(id);
    const known = [...this.data.items.map((x) => this.settings.getConnectorSecret(x.id)), this.settings.envSecret('githubToken'), this.settings.envSecret('anthropicApiKey'), this.settings.envSecret('openaiApiKey')];
    const out = await this.probe(r).catch((err: Error) => ({ ok: false, detail: err.message.split('\n')[0] ?? 'failed', status: undefined as ConnectorStatus | undefined }));
    if (out.status) this.statuses.set(id, out.status);
    return { ok: out.ok, detail: scrub(out.detail, known) };
  }

  private async probe(r: ConnectorRecord): Promise<{ ok: boolean; detail: string; status?: ConnectorStatus }> {
    const unavailable = { ok: false, detail: 'Connection tests are not available in this build.' };
    switch (r.kind) {
      case 'github': {
        const token = this.secretOf(r);
        if (!this.deps.github) return { ok: false, detail: 'GitHub is not available in this build.' };
        if (!token) return { ok: false, detail: 'Sign in with GitHub first.', status: 'signed-out' };
        try {
          const { login } = await this.deps.github.user(token);
          if (!this.isEnv(r) && login && (r as GithubRecord).login !== login) {
            (r as GithubRecord).login = login;
            this.settings.saveConnectors();
          }
          return { ok: true, detail: `signed in as ${login}`, status: 'connected' };
        } catch (err) {
          const msg = (err as Error).message;
          return { ok: false, detail: msg, ...(/\(401\)|rejected the sign-in/.test(msg) ? { status: 'expired' as const } : {}) };
        }
      }
      case 'aws': {
        if (!this.deps.tester) return unavailable;
        const res = await this.deps.tester.aws({ region: r.region, profile: r.profile });
        if (res.ok) return { ok: true, detail: res.detail, status: 'connected' };
        if (unreachable(res.detail)) return { ok: false, detail: res.detail };
        const auth = this.awsAuth(r.profile);
        const expired = auth === 'sso' || auth === 'role' || auth === 'process';
        return {
          ok: false,
          detail: auth === 'sso' ? `${res.detail} Run "aws sso login --profile ${r.profile}" in a terminal, then test again.` : res.detail,
          status: expired ? 'expired' : 'invalid',
        };
      }
      case 'agent': {
        if (r.source === 'login') {
          if (this.deps.harnesses) this.harness = (await this.deps.harnesses.refresh()).harnesses;
          const h = this.harnessOf(r.agent);
          const ok = !!h?.installed && h.auth !== 'signed-out';
          return { ok, detail: h?.hint ?? `${AGENT_LABEL[r.agent]} was not found on this Mac.`, status: ok ? 'connected' : 'signed-out' };
        }
        if (!this.deps.tester) return unavailable;
        const apiKey = this.secretOf(r);
        if (!apiKey) return { ok: false, detail: 'The key is not in the keychain any more; add it again.', status: 'invalid' };
        const res = await (r.agent === 'claude-code' ? this.deps.tester.anthropic({ apiKey }) : this.deps.tester.openai({ apiKey }));
        if (res.ok) return { ok: true, detail: res.detail, status: 'valid' };
        return { ok: false, detail: res.detail, ...(unreachable(res.detail) ? {} : { status: 'invalid' as const }) };
      }
    }
  }

  /** Probe the machine for agent CLIs again, then check every connector (failures never throw). */
  async refresh(): Promise<void> {
    if (this.deps.harnesses) {
      this.harness = (await this.deps.harnesses.refresh()).harnesses;
      this.addDetectedLogins();
    }
    await Promise.all(this.records().map((r) => (r.kind === 'agent' && r.source === 'login' ? Promise.resolve() : this.test(r.id).catch(() => undefined))));
  }

  // ------------------------------------------------------------------ secrets for the engine (main process only)

  /** GitHub token of a connector (default: the default GitHub connector). */
  githubToken(id?: string): string | null {
    const r = id ? this.find(id) : this.defaultOf('github');
    return r && r.kind === 'github' ? this.secretOf(r) : null;
  }

  /**
   * The value behind the old single-secret names: the default GitHub token, the Anthropic / OpenAI key of the default key connector
   * of that agent (or any), then the environment.
   */
  secretValue(name: SecretName): string | null {
    if (name === 'githubToken') return this.githubToken() ?? this.settings.envSecret('githubToken');
    const agent: ConnectorAgent = name === 'anthropicApiKey' ? 'claude-code' : 'codex';
    return this.keyOf(agent) ?? this.settings.envSecret(name);
  }
  hasSecret(name: SecretName): boolean {
    return this.secretValue(name) !== null;
  }

  /** The stored key of the best `api-key` connector for an agent: the default one if it is a key, else the first. */
  private keyOf(agent: ConnectorAgent): string | null {
    const keys = this.records().filter((r): r is AgentRecord => r.kind === 'agent' && r.agent === agent && r.source === 'api-key');
    const def = this.defaultId('agent');
    const pick = keys.find((r) => r.id === def) ?? keys.find((r) => this.statusOf(r) === 'valid') ?? keys[0];
    return pick ? this.secretOf(pick) : null;
  }

  // ------------------------------------------------------------------ per-repo selection

  private valid(sel: RepoConnections): { github: boolean; aws: boolean; agents: AgentSelection[] } {
    const ok = (id: string, kind: ConnectorKind) => this.find(id)?.kind === kind;
    const agents = sel.agents.filter((a) => {
      const r = this.find(a.connectorId);
      return r?.kind === 'agent' && (a.where === 'local' || r.source === 'api-key');
    });
    return { github: ok(sel.github, 'github'), aws: ok(sel.aws, 'aws'), agents };
  }

  /** The default connector of each kind as a selection (`null` = nothing connected of that kind). */
  private defaultSelection(): RepoConnectionsVM['effective'] {
    const agent = this.defaultId('agent');
    return { github: this.defaultId('github'), aws: this.defaultId('aws'), agents: agent ? [{ connectorId: agent, where: 'local' }] : [] };
  }

  /** What a run on this repo would use: the remembered choice where it is still valid, else the defaults. */
  repoConnections(repo: string): RepoConnectionsVM {
    const key = repoKey(repo);
    const saved = this.data.selections[key];
    const defaults = this.defaultSelection();
    if (!saved) return { repo: key, saved: null, effective: defaults };
    const v = this.valid(saved);
    const complete = v.github && v.aws && v.agents.length > 0;
    return {
      repo: key,
      saved: complete ? { github: saved.github, aws: saved.aws, agents: v.agents } : null,
      effective: { github: v.github ? saved.github : defaults.github, aws: v.aws ? saved.aws : defaults.aws, agents: v.agents.length > 0 ? v.agents : defaults.agents },
    };
  }

  /** Check and remember the choice for a repo. */
  saveRepoConnections(repo: string, sel: RepoConnections): RepoConnectionsVM {
    const gh = this.find(sel.github);
    if (gh?.kind !== 'github') throw new ConnectorError('Choose a GitHub account.');
    if (this.find(sel.aws)?.kind !== 'aws') throw new ConnectorError('Choose an AWS account.');
    if (sel.agents.length === 0) throw new ConnectorError('Choose at least one agent.');
    const seen = new Set<ConnectorAgent>();
    for (const a of sel.agents) {
      const r = this.find(a.connectorId);
      if (r?.kind !== 'agent') throw new ConnectorError('Choose an agent connector.');
      if (a.where === 'cloud' && r.source !== 'api-key') throw new ConnectorError(`${r.name} is a login on this Mac: it cannot run in the cloud. Use an API key.`);
      if (seen.has(r.agent)) throw new ConnectorError(`Choose one ${AGENT_LABEL[r.agent]} connector per run.`);
      seen.add(r.agent);
    }
    this.data.selections[repoKey(repo)] = { github: sel.github, aws: sel.aws, agents: sel.agents.map((a) => ({ connectorId: a.connectorId, where: a.where })) };
    this.settings.saveConnectors();
    return this.repoConnections(repo);
  }

  /** Where every chosen agent runs, or why the choice cannot be run (the engine runs all agents of one optimization in one place). */
  whereOf(agents: AgentSelection[]): ConnectorWhere | null {
    const places = new Set(agents.map((a) => a.where));
    if (places.size > 1) throw new ConnectorError('Run all agents on this Mac or all of them in the cloud: mixing is not supported yet.');
    return agents[0]?.where ?? null;
  }

  /**
   * The connectors a run on `repo` uses, with their secrets (`null` repo: the defaults). Backwards compatible: with nothing
   * remembered for the repo, the defaults are used, as the single-account Settings did.
   */
  async resolve(repo: string | null): Promise<ResolvedConnections> {
    await this.sync();
    const eff = repo ? this.repoConnections(repo).effective : this.defaultSelection();
    const gh = eff.github ? this.find(eff.github) : undefined;
    const aws = eff.aws ? this.find(eff.aws) : undefined;
    const token = gh?.kind === 'github' ? this.secretOf(gh) : null;
    const agents: ResolvedAgent[] = [];
    const cloudKeys: ResolvedConnections['cloudKeys'] = {};
    const saved = repo ? this.repoConnections(repo).saved : null;
    if (saved) {
      for (const a of saved.agents) {
        const r = this.find(a.connectorId) as AgentRecord;
        const apiKey = r.source === 'api-key' ? this.secretOf(r) : null;
        agents.push({ connectorId: r.id, name: r.name, agent: r.agent, source: r.source, where: a.where, apiKey });
        if (apiKey) cloudKeys[r.agent] = apiKey;
      }
    } else {
      // No choice for this repo: the best connector of each agent (the default one first), and any stored key for cloud runners.
      const def = this.defaultId('agent');
      for (const agent of AGENTS) {
        const mine = this.sortAgents(this.records().filter((r): r is AgentRecord => r.kind === 'agent' && r.agent === agent));
        const usable = mine.filter((r) => ['connected', 'valid'].includes(this.statusOf(r)));
        const pick = mine.find((r) => r.id === def) ?? usable[0] ?? mine[0];
        if (pick) agents.push({ connectorId: pick.id, name: pick.name, agent, source: pick.source, where: 'local', apiKey: pick.source === 'api-key' ? this.secretOf(pick) : null });
        const key = this.keyOf(agent);
        if (key) cloudKeys[agent] = key;
      }
    }
    return {
      github: gh?.kind === 'github' && token ? { id: gh.id, name: gh.name, login: gh.login || null, token } : null,
      aws: aws?.kind === 'aws' ? { id: aws.id, name: aws.name, profile: aws.profile, region: aws.region, monthlyCapUsd: aws.monthlyCapUsd } : null,
      agents,
      cloudKeys,
    };
  }
}
