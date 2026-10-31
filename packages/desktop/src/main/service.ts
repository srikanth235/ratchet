import { getTemplate, listTemplates, parseConfig, type EngineEvent, type Ledger } from '@ratchet/core';
import { validateLiveSetting, validateSecretFormat } from '../shared/live-settings.js';
import {
  type AddAgentConnectorInput,
  type AddAwsConnectorInput,
  type AddGithubConnectorInput,
  type ConnectorsVM,
  type ConnectorTestVM,
  type RepoConnections,
  type RepoConnectionsVM,
  type StartGithubSignInInput,
  type UpdateConnectorInput,
} from '../shared/connectors.js';
import type {
  AwsProfilesVM,
  ConnectionResultVM,
  ConnectionTarget,
  AgentKind,
  CreateOptimizationInput,
  EngineEventVM,
  EnvironmentVM,
  EnvironmentsVM,
  ExportReportResult,
  GithubStatusVM,
  HarnessVM,
  OptimizationsSnapshot,
  PreflightInput,
  PreflightItem,
  ProposeTemplateInput,
  ProposeTemplateVM,
  RatchetApi,
  RepoInspectVM,
  RepoSourceInput,
  RepoVM,
  SandboxPhaseVM,
  SecretName,
  SettingsVM,
  TemplateVM,
} from '../shared/types.js';
import { listAwsProfiles, type AwsProfileList } from './aws-profiles.js';
import { ConnectorError, ConnectorStore, type ConnectionTesterLike } from './connectors.js';
import type { AppEngine } from './dev-app.js';
import { slugOf } from './repo-slug.js';
import type { GithubApi } from './github.js';
import { GithubSignIn } from './github-signin.js';
import { toHarnessVM, type HarnessCache } from './harnesses.js';
import { effectivePaths, type LivePaths } from './live-config.js';
import { inspectLocalFolder, type LocalRepoInfo } from './local-repo.js';
import {
  agentEventsPage,
  buildActivity,
  buildCandidates,
  buildDetail,
  buildKpis,
  buildReport,
  buildSeries,
  buildSpend,
  toOptimization,
  toReceiptVM,
  toTemplateVM,
  toWinningChange,
  validateConfigText,
  type MetaView,
} from './mappers.js';
import { buildOverview } from './overview.js';
import type { SettingsStore } from './settings.js';

const DEFAULT_REGION = 'us-east-1';
const DEFAULT_IDLE_MINUTES = 30;

export interface ServiceDeps {
  ledger: Ledger;
  settings: SettingsStore;
  /** Builds the engine; called at startup. `configFor` hands it the parsed ratchet.yaml of repos seen in step 1 (region). */
  createApp: (hooks: { configFor(slug: string): unknown; connectors: ConnectorStore }) => AppEngine;
  now?: () => number;
  /** Ask the user where to save the report; resolves to the written path, or null if cancelled. */
  saveReport?: (defaultName: string, markdown: string) => Promise<string | null>;
  /** Backs "Test connection" in Settings (@ratchet/runtime createConnectionTester); without it the tests report unavailable. */
  connectionTester?: ConnectionTesterLike;
  /** What empty tofu settings resolve to (the bundled infra/aws in a packaged app). */
  liveDefaults?: LivePaths;
  /** GitHub REST + device flow. Without it GitHub features report that they are unavailable. */
  github?: GithubApi;
  /** Open a URL in the system browser (the device-flow verification page). */
  openExternal?: (url: string) => void;
  /** Injectable pause for the device-flow poll loop (tests). */
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
  /** The AWS profiles on this machine; default: parse ~/.aws. */
  listAwsProfiles?: () => AwsProfileList;
  /** The detected agent CLIs. Without it nothing is detected. */
  harnesses?: HarnessCache;
  /** Native folder dialog for "Local folder…". */
  pickFolder?: () => Promise<string | null>;
  /** Read a local folder (GitHub remote, default branch, ratchet.yaml). */
  inspectLocal?: (path: string) => Promise<LocalRepoInfo>;
  /** Idle window of the automatic environment, minutes (shown in Environments). */
  idleMinutes?: number;
}

export type { ConnectionTesterLike } from './connectors.js';

export type RatchetService = Omit<RatchetApi, 'onEvent'> & {
  onEvent(listener: (e: EngineEventVM) => void): () => void;
  dispose(): void;
};

const demoName = (id: string): string => getTemplate(id)?.title.replace(/^Demo:\s*/i, '').replace(/^./, (c) => c.toUpperCase()) + ' (demo)';

/** Everything the renderer can ask for, backed by the ledger (reads) and `EngineApi` (commands). */
export function createService(deps: ServiceDeps): RatchetService {
  const { ledger, settings } = deps;
  const now = deps.now ?? Date.now;
  const listeners = new Set<(e: EngineEventVM) => void>();
  const configs = new Map<string, unknown>();
  const phases = new Map<string, { phase: SandboxPhaseVM; region: string | null; text: string }>();
  const github = deps.github;
  const awsProfiles = (): AwsProfileList => (deps.listAwsProfiles ?? listAwsProfiles)();
  const connectors = new ConnectorStore(settings, {
    github,
    harnesses: deps.harnesses,
    listAwsProfiles: awsProfiles,
    tester: deps.connectionTester,
    now,
  });
  const app = deps.createApp({ configFor: (slug) => configs.get(slug), connectors });
  const unsubscribe = app.engine.subscribe((e) => {
    if (e.type === 'sandbox') phases.set(e.sandboxId, { phase: e.phase, region: e.region, text: e.text });
    const vm = toEventVM(e);
    for (const l of listeners) l(vm);
  });
  const liveDefaults: LivePaths = deps.liveDefaults ?? { tofuBinary: 'tofu', moduleDir: 'infra/aws' };
  const signIn = github
    ? new GithubSignIn({
        api: github,
        save: (token, login, connectorId) => {
          connectors.upsertGithub({ login, token, source: 'device', ...(connectorId ? { connectorId } : {}) });
          envLogin = null;
        },
        ...(deps.openExternal ? { openExternal: deps.openExternal } : {}),
        ...(deps.sleep ? { sleep: deps.sleep } : {}),
        now,
        onSignedIn: () => app.invalidate?.(),
      })
    : null;
  /** The login of a token that came from the environment (no stored account). */
  let envLogin: { token: string; login: string | null } | null = null;

  const meta: MetaView = { prUrl: (id) => settings.prUrl(id) };
  const engineInfo = () => ({ kind: app.kind, mode: app.engine.mode() });
  const idleMinutes = deps.idleMinutes ?? DEFAULT_IDLE_MINUTES;

  /** The default AWS connector: what the single-profile Settings used to hold. */
  const defaultAws = () => {
    const r = connectors.defaultOf('aws');
    return r?.kind === 'aws' ? r : null;
  };
  const awsProfileName = (): string => defaultAws()?.profile ?? '';
  const awsRegionName = (): string => defaultAws()?.region ?? (settings.live.awsRegion || DEFAULT_REGION);

  const settingsVM = (): SettingsVM => ({
    awsProfile: awsProfileName(),
    awsRegion: awsRegionName(),
    secrets: {
      anthropicApiKey: connectors.hasSecret('anthropicApiKey'),
      openaiApiKey: connectors.hasSecret('openaiApiKey'),
      githubToken: connectors.hasSecret('githubToken'),
    },
    dev: { ...settings.live, awsProfile: awsProfileName(), awsRegion: awsRegionName(), defaults: { tofuBinary: effectivePaths(settings.live, liveDefaults).tofuBinary, moduleDir: effectivePaths(settings.live, liveDefaults).moduleDir } },
    encryptionAvailable: settings.encryptionAvailable,
    engine: engineInfo(),
  });

  /** The token of a GitHub connector (default: the default one). */
  const needToken = (connectorId?: string): string => {
    const token = connectors.githubToken(connectorId) ?? (connectorId ? null : settings.envSecret('githubToken'));
    if (!token) throw new Error('Sign in with GitHub first (Settings).');
    return token;
  };
  const needGithub = (): GithubApi => {
    if (!github) throw new Error('GitHub is not available in this build.');
    return github;
  };

  /** The default GitHub account, as the old single-account row showed it. */
  async function githubStatus(): Promise<GithubStatusVM> {
    const rec = connectors.defaultOf('github');
    const token = connectors.githubToken();
    const flow = signIn?.current() ?? null;
    const clientIdConfigured = github?.clientIdConfigured ?? false;
    if (!token || rec?.kind !== 'github') return { signedIn: false, login: null, source: null, clientIdConfigured, flow };
    let login = rec.login || null;
    const source = rec.id === 'github-env' ? 'env' : rec.source;
    if (!login && github) {
      if (envLogin?.token !== token) {
        envLogin = { token, login: await github.user(token).then((u) => u.login, () => null) };
      }
      login = envLogin.login;
    }
    return { signedIn: true, login, source, clientIdConfigured, flow };
  }

  async function environments(): Promise<EnvironmentVM[]> {
    const rows = ledger.listSandboxes().filter((r) => r.status !== 'destroyed' && r.status !== 'planned');
    const out: EnvironmentVM[] = [];
    for (const r of rows) {
      const act = await app.engine.sandboxActivity?.(r.id).catch(() => null);
      const seen = phases.get(r.id);
      const derived: SandboxPhaseVM = r.status === 'applying' ? 'creating' : r.status === 'destroying' ? 'destroying' : r.status === 'failed' ? 'failed' : (act?.activeRuns.length ?? 0) > 0 ? 'ready' : 'idle';
      const phase = seen && (seen.phase === 'failed' || seen.phase === 'destroying' || seen.phase === 'creating') ? seen.phase : derived;
      out.push({
        sandboxId: r.id,
        region: act?.region ?? r.region ?? seen?.region ?? null,
        phase,
        text: seen?.text ?? (phase === 'idle' ? 'idle: no run is active' : phase === 'ready' ? 'environment ready' : phase),
        simulated: r.provider === 'mock',
        activeRuns: act?.activeRuns ?? [],
        instancesRunning: act?.instancesRunning ?? null,
        spendUsd: act?.spendUsd ?? 0,
        destroyAt: act?.destroyAt ?? null,
        idleSince: act?.idleSince ?? null,
        canEnd: r.status === 'applied' && (act?.activeRuns.length ?? 0) === 0 && !!app.engine.endSandbox,
      });
    }
    return out;
  }

  async function environmentsVM(): Promise<EnvironmentsVM> {
    const envs = await environments();
    let last: Awaited<ReturnType<NonNullable<typeof app.engine.sandboxActivity>>>['lastReceipt'] = null;
    for (const r of ledger.listSandboxes()) {
      const act = await app.engine.sandboxActivity?.(r.id).catch(() => null);
      if (act?.lastReceipt && (!last || act.lastReceipt.destroyedAt > last.destroyedAt)) last = act.lastReceipt;
    }
    return { environments: envs, lastReceipt: last ? toReceiptVM(last) : null, idleMinutes };
  }

  /** Parse a repo's ratchet.yaml for step 2 and remember its config for the region. */
  function inspectResult(source: RepoSourceInput, repo: string | null, baseRef: string, configYaml: string | null, problem: string | null): RepoInspectVM {
    let config: RepoInspectVM['config'] = null;
    let errors: RepoInspectVM['errors'] = [];
    if (configYaml !== null) {
      const v = validateConfigText(configYaml);
      if (v.ok) {
        config = v.summary;
        const parsed = parseConfig(configYaml);
        if (parsed.ok && repo) configs.set(repo, parsed.config);
      } else errors = v.errors;
    }
    const templates = configYaml === null && problem === null ? listTemplates({ kind: 'template' }).map(toTemplateVM) : [];
    return { source, repo, baseRef, configYaml, config, errors, templates, problem };
  }

  async function preflight({ agents, agentsWhere, source, repo }: PreflightInput): Promise<PreflightItem[]> {
    if (source === 'demo') {
      return [{ id: 'demo', label: 'Demo run', state: 'ok', detail: 'simulation: scripted agents, mock AWS, nothing is installed or billed' }];
    }
    const items: PreflightItem[] = [];
    // The connectors this repo's runs use: its remembered choice, else the defaults.
    const chosen = await connectors.resolve(repo ?? null);
    items.push(
      chosen.github
        ? { id: 'github', label: 'GitHub', state: 'ok', detail: chosen.github.login ? `signed in as ${chosen.github.login}` : 'signed in' }
        : { id: 'github', label: 'Sign in with GitHub', state: 'fail', detail: 'Ratchet pushes candidate branches and opens the pull request as you' },
    );
    items.push(
      chosen.aws
        ? { id: 'aws', label: 'AWS', state: 'ok', detail: `${chosen.aws.profile} · ${chosen.aws.region}` }
        : { id: 'aws', label: 'Connect AWS', state: 'fail', detail: 'the evaluator runs in a sandbox in your AWS account' },
    );
    if (agentsWhere === 'local') {
      const found = deps.harnesses ? (await deps.harnesses.get()).harnesses : [];
      const kinds = agents.filter((a): a is 'claude-code' | 'codex' => a === 'claude-code' || a === 'codex');
      if (kinds.length === 0) items.push({ id: 'agents', label: 'Choose an agent', state: 'fail', detail: 'no agent is selected' });
      for (const k of kinds) {
        const h = found.find((x) => x.kind === k);
        const label = k === 'codex' ? 'Codex' : 'Claude Code';
        if (h && h.installed && h.auth !== 'signed-out') items.push({ id: `agent-${k}`, label, state: 'ok', detail: `${h.version ?? 'installed'} · ${h.auth === 'api-key' ? 'API key' : 'signed in'}` });
        else items.push({ id: `agent-${k}`, label: `${label} is not ready`, state: 'fail', detail: h?.hint ?? `install ${label} on this machine` });
      }
    } else {
      for (const a of agents) {
        const name: SecretName | null = a === 'claude-code' ? 'anthropicApiKey' : a === 'codex' ? 'openaiApiKey' : null;
        const key = a === 'claude-code' || a === 'codex' ? chosen.cloudKeys[a] : undefined;
        if (name && !key && !settings.envSecret(name)) items.push({ id: `key-${a}`, label: `${a === 'codex' ? 'OpenAI' : 'Anthropic'} API key`, state: 'fail', detail: 'cloud runners need an API key (Connectors)' });
        else if (name) items.push({ id: `key-${a}`, label: `${a === 'codex' ? 'OpenAI' : 'Anthropic'} API key`, state: 'ok', detail: 'stored in the keychain' });
      }
    }
    return items;
  }

  async function testConnection(target: ConnectionTarget): Promise<ConnectionResultVM> {
    const t = deps.connectionTester;
    const aws = defaultAws();
    const result = ((): Promise<ConnectionResultVM> => {
      const need = (what: string): Promise<ConnectionResultVM> => Promise.resolve({ target, ok: false, detail: what });
      if (target === 'github') {
        const token = connectors.secretValue('githubToken');
        if (!github) return need('GitHub is not available in this build.');
        if (!token) return need('Sign in with GitHub first.');
        return github.user(token).then((u) => ({ target, ok: true, detail: `signed in as ${u.login}` }));
      }
      if (!t) return need('Connection tests are not available in this build.');
      switch (target) {
        case 'aws':
          if (!aws) return need('Choose an AWS profile first.');
          return t.aws({ region: aws.region, profile: aws.profile }).then((x) => ({ target, ...x }));
        case 'anthropic': {
          const apiKey = connectors.secretValue('anthropicApiKey');
          return apiKey ? t.anthropic({ apiKey }).then((x) => ({ target, ...x })) : need('Store an Anthropic API key first.');
        }
        case 'openai': {
          const apiKey = connectors.secretValue('openaiApiKey');
          return apiKey ? t.openai({ apiKey }).then((x) => ({ target, ...x })) : need('Store an OpenAI API key first.');
        }
      }
    })();
    let r: ConnectionResultVM;
    try {
      r = await result;
    } catch (err) {
      r = { target, ok: false, detail: (err as Error).message.split('\n')[0] ?? 'failed' };
    }
    // Belt and braces: no stored secret value may ever leave in a message, whatever the tester returned.
    let detail = r.detail;
    for (const name of ['anthropicApiKey', 'openaiApiKey', 'githubToken'] as SecretName[]) {
      const v = connectors.secretValue(name);
      if (v && v.length >= 4) detail = detail.split(v).join('***');
    }
    if (target === 'aws' && !r.ok && aws && awsProfiles().profiles.find((p) => p.name === aws.profile)?.kind === 'sso') {
      detail += ` Run "aws sso login --profile ${aws.profile}" in a terminal, then test again.`;
    }
    return { target, ok: r.ok, detail };
  }

  // ---------------------------------------------------------------- connectors

  async function connectorsVM(): Promise<ConnectorsVM> {
    return {
      connectors: await connectors.list(),
      githubFlow: signIn?.current() ?? null,
      encryptionAvailable: settings.encryptionAvailable,
      clientIdConfigured: github?.clientIdConfigured ?? false,
    };
  }
  /** A connector changed: runtimes built from the old choice are rebuilt on next use. */
  async function changed(): Promise<ConnectorsVM> {
    app.invalidate?.();
    return connectorsVM();
  }

  /** The single-profile call of the first Settings: the profile becomes the default AWS connector (added when it is new). */
  function setProfile(profile: string): void {
    if (profile === '') {
      const def = defaultAws();
      if (def) connectors.remove(def.id);
      return;
    }
    const existing = connectors.snapshot().find((c) => c.kind === 'aws' && c.profile === profile);
    if (existing) connectors.setDefault(existing.id);
    else connectors.addAws({ profile, region: settings.live.awsRegion || DEFAULT_REGION, makeDefault: true });
    // The auth kind follows the chosen profile (SSO sessions need `aws sso login`).
    const kind = awsProfiles().profiles.find((p) => p.name === profile)?.kind;
    settings.setLive({ awsAuth: kind === 'sso' ? 'sso' : 'profile' });
  }

  return {
    async listOptimizations(opts): Promise<OptimizationsSnapshot> {
      const t = now();
      const range = opts?.range ?? '30d';
      const rows = ledger.listOptimizations();
      const optimizations = rows.map((r) => toOptimization(r, ledger.getOptimization(r.id)!, meta, t));
      const featured = rows.find((r) => r.status === 'running' || r.status === 'baselining') ?? rows.find((r) => r.baselineValue !== null) ?? null;
      return {
        range,
        kpis: buildKpis(ledger, rows, t, range),
        featuredId: featured?.id ?? null,
        featuredSeries: featured ? buildSeries(ledger, featured.id, t) : null,
        optimizations,
        activity: buildActivity(ledger, t, 8),
        engine: engineInfo(),
      };
    },
    async overview(range) {
      return buildOverview(ledger, meta, now(), range);
    },
    async getOptimization(id) {
      const d = buildDetail(ledger, id, meta, now());
      const [receipt, calibration] = await Promise.all([
        app.engine.teardownReceipt?.(id).catch(() => null) ?? null,
        app.engine.calibration?.(id).catch(() => null) ?? null,
      ]);
      return { ...d, receipt: receipt ? toReceiptVM(receipt) : null, calibration: calibration?.explanation ?? null };
    },
    async ratchetHistory(id, metric) {
      return buildSeries(ledger, id, now(), metric);
    },
    async winningChange(id) {
      const opt = ledger.getOptimization(id);
      if (!opt) throw new Error(`optimization ${id} not found`);
      const best = ledger.ratchetHistory(id).at(-1);
      if (!best) return toWinningChange(ledger, id, null, null);
      if (!app.diff) return toWinningChange(ledger, id, null, 'This engine has no git access to diff the winning commit.');
      try {
        const files = await app.diff({ optimizationId: id, repo: opt.repo, baseRef: opt.baseRef, sha: best.sha });
        return toWinningChange(ledger, id, files, files ? null : `No diff available for ${best.sha.slice(0, 7)}.`);
      } catch (err) {
        return toWinningChange(ledger, id, null, `Could not diff ${best.sha.slice(0, 7)} against ${opt.baseRef}: ${(err as Error).message.split('\n')[0]}`);
      }
    },
    async candidatesWithResults(id) {
      return buildCandidates(ledger, id);
    },
    async spendBreakdown(id) {
      return buildSpend(ledger, id);
    },
    async recentActivity(opts) {
      return buildActivity(ledger, now(), opts?.limit ?? 20, opts?.optimizationId);
    },
    async agentEvents(agentId, cursor, limit) {
      return agentEventsPage(ledger, agentId, cursor, limit);
    },
    async validateConfig(text) {
      return validateConfigText(text);
    },
    preflight,
    async getSettings() {
      return settingsVM();
    },
    environments: environmentsVM,

    // ------------------------------------------------------------ GitHub
    githubStatus,
    async githubSignIn() {
      await needSignIn().start();
      return githubStatus();
    },
    async githubSignOut() {
      signIn?.cancel();
      const rec = connectors.defaultOf('github');
      if (rec && rec.id !== 'github-env') connectors.remove(rec.id);
      envLogin = null;
      app.invalidate?.();
      return githubStatus();
    },
    async listRepos(query, connectorId): Promise<RepoVM[]> {
      return needGithub().repos(needToken(connectorId), query);
    },
    async inspectRepo(source, connectorId): Promise<RepoInspectVM> {
      if (source.kind === 'github') {
        const token = needToken(connectorId);
        const gh = needGithub();
        const repo = await gh.repo(token, source.repo);
        const configYaml = await gh.file(token, repo.fullName, repo.defaultBranch, 'ratchet.yaml');
        return inspectResult(source, repo.fullName, repo.defaultBranch, configYaml, null);
      }
      const info = await (deps.inspectLocal ?? inspectLocalFolder)(source.path);
      return inspectResult(source, info.repo, info.baseRef, info.configYaml, info.problem);
    },
    async pickLocalFolder() {
      return deps.pickFolder ? deps.pickFolder() : null;
    },

    // ------------------------------------------------------------ AWS and agents
    async awsProfiles(): Promise<AwsProfilesVM> {
      const found = awsProfiles();
      return {
        profiles: found.profiles,
        ssoSessions: found.ssoSessions,
        selected: awsProfileName(),
        region: awsRegionName(),
        defaultRegion: DEFAULT_REGION,
        filesFound: found.filesFound,
      };
    },
    async detectHarnesses(): Promise<HarnessVM[]> {
      return deps.harnesses ? (await deps.harnesses.get()).harnesses.map(toHarnessVM) : [];
    },
    async redetectHarnesses(): Promise<HarnessVM[]> {
      if (!deps.harnesses) return [];
      const r = await deps.harnesses.refresh();
      app.invalidate?.();
      return r.harnesses.map(toHarnessVM);
    },

    // ------------------------------------------------------------ connectors
    listConnectors: connectorsVM,
    async refreshConnectors() {
      await connectors.refresh();
      return changed();
    },
    async addGithubConnector(input: AddGithubConnectorInput) {
      await connectors.addGithubToken(input.token, input.name);
      envLogin = null;
      return changed();
    },
    async startGithubConnectorSignIn(input?: StartGithubSignInInput) {
      if (input?.connectorId && !connectors.vmOf(input.connectorId)) throw new ConnectorError('That connector no longer exists.');
      await needSignIn().start(input?.connectorId ? { connectorId: input.connectorId } : {});
      return connectorsVM();
    },
    async cancelGithubConnectorSignIn() {
      signIn?.cancel();
      return connectorsVM();
    },
    async addAwsConnector(input: AddAwsConnectorInput) {
      const known = awsProfiles();
      if (known.filesFound && !known.profiles.some((p) => p.name === input.profile.trim())) throw new ConnectorError(`${input.profile} is not in ~/.aws/config or ~/.aws/credentials.`);
      const rec = connectors.addAws(input);
      void connectors.test(rec.id).catch(() => undefined); // the screen refreshes the status; an offline probe must not hold the add
      return changed();
    },
    async addAgentConnector(input: AddAgentConnectorInput) {
      await connectors.addAgent(input, { waitForCheck: false });
      return changed();
    },
    async updateConnector(id: string, patch: UpdateConnectorInput) {
      connectors.update(id, patch);
      return changed();
    },
    async removeConnector(id: string) {
      connectors.remove(id);
      envLogin = null;
      return changed();
    },
    async setDefaultConnector(id: string) {
      connectors.setDefault(id);
      return changed();
    },
    async testConnector(id: string): Promise<ConnectorTestVM> {
      const r = await connectors.test(id);
      return { ...r, connector: connectors.vmOf(id) };
    },
    async getRepoConnections(repo: string): Promise<RepoConnectionsVM> {
      await connectors.list(); // detected CLI logins are connectors too
      return connectors.repoConnections(repo);
    },
    async saveRepoConnections(repo: string, selection: RepoConnections): Promise<RepoConnectionsVM> {
      const r = connectors.saveRepoConnections(repo, selection);
      app.invalidate?.();
      return r;
    },

    // ------------------------------------------------------------ templates
    async listTemplates(): Promise<TemplateVM[]> {
      return listTemplates().map(toTemplateVM);
    },
    async proposeTemplate(input: ProposeTemplateInput): Promise<ProposeTemplateVM> {
      if (!app.engine.proposeTemplate) throw new Error('This engine cannot open template pull requests.');
      needToken();
      const slug = slugOf(input.repoUrl);
      if (!slug) throw new Error(`${input.repoUrl} is not a GitHub repository (owner/name).`);
      const r = await app.engine.proposeTemplate({ ...input, repoUrl: slug });
      return { prUrl: r.prUrl, branch: r.branch, sha: r.sha, templateId: r.templateId, files: r.files };
    },

    // ------------------------------------------------------------ commands
    async createOptimization(input: CreateOptimizationInput) {
      const base = {
        agents: input.agents,
        budgetUsd: input.budgetUsd,
        timeLimitSeconds: input.timeLimitSeconds,
        plateauEvals: input.plateauEvals,
      };
      if (input.demo) {
        const demoInput = { ...base, name: input.name ?? demoName(input.demo), repoUrl: `acme/${input.demo}`, baseRef: 'main' };
        return app.createDemo ? app.createDemo(input.demo, demoInput) : app.engine.createOptimization(demoInput);
      }
      const slug = slugOf(input.repoUrl);
      if (!slug) throw new Error(`${input.repoUrl} is not a GitHub repository (owner/name).`);
      // The connectors picked for this run become the repo's remembered choice, and the runtime for the repo is built from them.
      let agentsWhere = input.agentsWhere;
      let agents = input.agents;
      if (input.connections) {
        connectors.saveRepoConnections(slug, input.connections);
        agentsWhere = connectors.whereOf(input.connections.agents) ?? agentsWhere;
        agents = [...new Set(input.connections.agents.map((a) => (connectors.vmOf(a.connectorId) as { agent: AgentKind }).agent))];
        app.invalidate?.();
      }
      const missing = (await app.missingFor?.(slug)) ?? [];
      if (missing.length > 0) throw new Error(`Not ready: ${missing.join('; ')}`);
      return app.engine.createOptimization({
        ...base,
        agents,
        ...(input.name ? { name: input.name } : {}),
        repoUrl: slug,
        baseRef: input.baseRef,
        ...(agentsWhere ? { agentsWhere } : {}),
      });
    },
    async startOptimization(id) {
      await app.engine.start(id);
    },
    async stopOptimization(id, reason) {
      await app.engine.stop(id, reason);
    },
    async pauseAgent(agentId) {
      await app.engine.pauseAgent(agentId);
    },
    async resumeAgent(agentId) {
      await app.engine.resumeAgent(agentId);
    },
    async messageAgent(agentId, text) {
      await app.engine.messageAgent(agentId, text);
    },
    async openPullRequest(id) {
      const pr = await app.engine.openPullRequest(id);
      settings.setPrUrl(id, pr.url);
      return pr;
    },
    async exportReport(id): Promise<ExportReportResult> {
      const md = buildReport(ledger, id, meta, now());
      const name = `ratchet-report-${id}.md`;
      const path = deps.saveReport ? await deps.saveReport(name, md) : null;
      return path ? { saved: true, path } : { saved: false };
    },
    async endSandbox(sandboxId) {
      if (!app.engine.endSandbox) throw new Error('This engine cannot end the sandbox.');
      await app.engine.endSandbox(sandboxId);
      return environmentsVM();
    },

    // ------------------------------------------------------------ settings
    async setAwsProfile(profile) {
      setProfile(profile);
      app.invalidate?.();
      return settingsVM();
    },
    async setSecret(name: SecretName, value) {
      const problem = validateSecretFormat(name, value);
      if (problem) throw new Error(problem);
      if (name === 'githubToken') {
        // A pasted token: prove it works and learn the account before keeping it; it becomes the default GitHub account.
        await connectors.addGithubToken(value, undefined, true);
        envLogin = null;
      } else {
        // The single key of the first Settings: replaces the agent's stored key connector (or adds one).
        const agent = name === 'anthropicApiKey' ? 'claude-code' : 'codex';
        for (const c of connectors.snapshot()) if (c.kind === 'agent' && c.agent === agent && c.source === 'api-key') connectors.remove(c.id);
        await connectors.addAgent({ agent, source: 'api-key', apiKey: value });
      }
      app.invalidate?.();
      return settingsVM();
    },
    async clearSecret(name: SecretName) {
      if (name === 'githubToken') {
        const rec = connectors.defaultOf('github');
        if (rec && rec.id !== 'github-env') connectors.remove(rec.id);
      } else {
        const agent = name === 'anthropicApiKey' ? 'claude-code' : 'codex';
        for (const c of connectors.snapshot()) if (c.kind === 'agent' && c.agent === agent && c.source === 'api-key') connectors.remove(c.id);
      }
      app.invalidate?.();
      return settingsVM();
    },
    async saveLiveSettings(input) {
      const { awsProfile, ...rest } = input;
      const bad = awsProfile !== undefined ? validateLiveSetting('awsProfile', awsProfile) : null;
      if (bad) throw new Error(`AWS profile: ${bad}`);
      settings.setLive(rest); // validates the whole batch first
      if (awsProfile !== undefined) setProfile(awsProfile.trim());
      const def = defaultAws();
      if (rest.awsRegion && def) connectors.update(def.id, { region: rest.awsRegion.trim() });
      app.invalidate?.();
      return settingsVM();
    },
    async testConnection(target: ConnectionTarget): Promise<ConnectionResultVM> {
      return testConnection(target);
    },

    onEvent(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    dispose() {
      signIn?.cancel();
      unsubscribe();
      app.dispose();
      listeners.clear();
    },
  };

  function needSignIn(): GithubSignIn {
    if (!signIn) throw new Error('GitHub is not available in this build.');
    return signIn;
  }
}

function toEventVM(e: EngineEvent): EngineEventVM {
  switch (e.type) {
    case 'agent-event':
      return { type: 'agent-event', optimizationId: e.optimizationId, agentId: e.agentId };
    case 'candidate':
      return { type: 'candidate', optimizationId: e.optimizationId, candidateId: e.candidateId, verdict: e.verdict };
    default:
      return e;
  }
}
