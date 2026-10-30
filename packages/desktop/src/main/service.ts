import type { EngineEvent, Ledger, SandboxInput, SandboxOutputs, SandboxPlan } from '@ratchet/core';
import type { AppEngine } from './engine.js';
import { sandboxIdFor } from './constants.js';
import { effectivePaths, liveReadiness, sandboxSetupProblems, type LivePaths } from './live-config.js';
import { validateSecretFormat } from '../shared/live-settings.js';
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
  toWinningChange,
  validateConfigText,
  type MetaView,
} from './mappers.js';
import type { SettingsStore } from './settings.js';
import type {
  AgentKind,
  ConnectionResultVM,
  ConnectionTarget,
  CreateOptimizationInput,
  EngineEventVM,
  ExportReportResult,
  Mode,
  OptimizationsSnapshot,
  LiveSettingsInput,
  PreflightItem,
  RatchetApi,
  SandboxInfo,
  SecretName,
  SettingsVM,
} from '../shared/types.js';

/** The live sandbox id (kept for importers); simulation uses its own, see `sandboxIdFor`. */
export { SANDBOX_IDS, sandboxIdFor } from './constants.js';
export const SANDBOX_ID = sandboxIdFor('live');
const DEFAULT_INSTANCE_TYPES = ['c7i.2xlarge', 'c7i.xlarge', 'm7i.large'];

export interface ServiceDeps {
  ledger: Ledger;
  settings: SettingsStore;
  /** Builds the engine for a mode; called at startup and on every mode switch. */
  createApp: (mode: Mode) => AppEngine;
  now?: () => number;
  /** Ask the user where to save the report; resolves to the written path, or null if cancelled. */
  saveReport?: (defaultName: string, markdown: string) => Promise<string | null>;
  /** Backs "Test connection" in Settings (@ratchet/runtime createConnectionTester); without it the tests report unavailable. */
  connectionTester?: ConnectionTesterLike;
  /** What empty tofu settings resolve to (the bundled infra/aws in a packaged app). */
  liveDefaults?: LivePaths;
}

/** The slice of @ratchet/runtime's ConnectionTester the service uses (kept structural so renderer tests need no AWS SDK). */
export interface ConnectionTesterLike {
  aws(i: { region: string; profile?: string | undefined; expectedAccountId?: string | undefined }): Promise<ConnectionResultVM>;
  github(i: { repo: string; token: string }): Promise<ConnectionResultVM>;
  anthropic(i: { apiKey: string }): Promise<ConnectionResultVM>;
  openai(i: { apiKey: string }): Promise<ConnectionResultVM>;
  tofu(i: { binary: string; moduleDir: string }): Promise<ConnectionResultVM>;
}

export type RatchetService = Omit<RatchetApi, 'onEvent'> & {
  onEvent(listener: (e: EngineEventVM) => void): () => void;
  dispose(): void;
};

/** Everything the renderer can ask for, backed by the ledger (reads) and `EngineApi` (commands). */
export function createService(deps: ServiceDeps): RatchetService {
  const { ledger, settings } = deps;
  const now = deps.now ?? Date.now;
  const listeners = new Set<(e: EngineEventVM) => void>();
  let app = deps.createApp(settings.mode);
  let unsubscribe = wire(app);
  const liveDefaults: LivePaths = deps.liveDefaults ?? { tofuBinary: 'tofu', moduleDir: 'infra/aws' };
  /** The last plan the user asked for (live) or the free one (simulation), per sandbox; dropped after apply / destroy. */
  const plans = new Map<string, SandboxPlan>();
  /** Settings changed while an optimization was running: the engine is rebuilt as soon as none is. */
  let rebuildPending = false;

  const meta: MetaView = { prUrl: (id) => settings.prUrl(id), teardownDone: (id) => settings.tornDown(id) };

  function wire(a: AppEngine): () => void {
    return a.engine.subscribe((e) => {
      const vm = toEventVM(e);
      for (const l of listeners) l(vm);
    });
  }

  const engineInfo = () => ({ kind: app.kind, mode: app.engine.mode() });

  const sandboxApplied = (): boolean => ledger.getSandbox(sandboxIdFor('live'))?.status === 'applied';

  const settingsVM = (): SettingsVM => {
    const readiness = liveReadiness({ live: settings.live, hasSecret: (n) => settings.hasSecret(n), sandboxApplied: sandboxApplied(), defaults: liveDefaults });
    return {
      mode: settings.mode,
      awsProfile: settings.awsProfile,
      secrets: {
        anthropicApiKey: settings.hasSecret('anthropicApiKey'),
        openaiApiKey: settings.hasSecret('openaiApiKey'),
        githubToken: settings.hasSecret('githubToken'),
      },
      live: { ...settings.live, defaults: liveDefaults, readiness, ready: readiness.every((r) => r.state !== 'fail'), restartRequired: rebuildPending },
      encryptionAvailable: settings.encryptionAvailable,
      engine: engineInfo(),
    };
  };

  /** Throw away the engine and build it again from the current settings (mode switch, new credentials, new outputs). */
  function rebuild(): void {
    unsubscribe();
    app.dispose();
    plans.clear();
    app = deps.createApp(settings.mode);
    unsubscribe = wire(app);
    rebuildPending = false;
  }

  /**
   * Live engines are built from Settings and the applied sandbox's outputs, so they are rebuilt when those change.
   * Rebuilding stops the control loop, so while an optimization is running it waits until none is.
   */
  function rebuildLiveEngine(): void {
    if (settings.mode !== 'live') {
      rebuildPending = false;
      return;
    }
    if (ledger.listOptimizations({ status: ['baselining', 'running', 'stopping'] }).length > 0) {
      rebuildPending = true;
      return;
    }
    rebuild();
  }

  const currentSandboxId = (): string => sandboxIdFor(settings.mode);

  const sandboxInput = (types: string[] = DEFAULT_INSTANCE_TYPES): SandboxInput => ({
    sandboxId: currentSandboxId(),
    region: settings.live.awsRegion || 'us-east-1',
    allowedInstanceTypes: types,
    maxInstancesPerAgent: 2,
    maxInstanceLifetimeHours: 4,
    tags: { 'ratchet:managed': 'true' },
  });

  const instanceTypes = (): string[] => {
    const latest = ledger.listOptimizations()[0];
    const cfg = latest ? validateConfigText(ledger.getOptimization(latest.id)!.configYaml) : null;
    return cfg?.ok ? cfg.summary.instanceTypes : DEFAULT_INSTANCE_TYPES;
  };

  const isSimulated = (): boolean => app.kind === 'dev' || settings.mode === 'simulation';

  const setupProblems = (): string[] => (settings.mode === 'live' ? sandboxSetupProblems(settings.live, liveDefaults) : []);

  async function buildSandboxInfo(): Promise<SandboxInfo> {
    const input = sandboxInput(instanceTypes());
    const simulated = isSimulated();
    const isBusy = (st: string): boolean => st === 'applying' || st === 'destroying';
    // Planning is free in simulation, so it is always current; a real `tofu plan` runs only when the user asks for it.
    if (simulated && !isBusy((await app.engine.sandboxState(input.sandboxId)).status)) {
      try {
        plans.set(input.sandboxId, await app.engine.planSandbox(input));
      } catch {
        /* shown as "no plan yet" */
      }
    }
    const state = await app.engine.sandboxState(input.sandboxId); // after planning: the first plan records the sandbox
    const busy = isBusy(state.status);
    const problems = setupProblems();
    const outputs: SandboxOutputs | null = state.status === 'applied' ? state.outputs : null;
    const active = state.optimizations.filter((o) => ['baselining', 'running', 'stopping'].includes(o.status));
    let destroyBlockedReason: string | null = null;
    if (state.status !== 'applied' && state.status !== 'failed') destroyBlockedReason = state.status === 'applying' || state.status === 'destroying' ? `The sandbox is ${state.status}.` : 'There is nothing to destroy.';
    else if (active.length > 0) destroyBlockedReason = `Stop ${active.length === 1 ? `optimization ${active[0]!.id}` : `${active.length} running optimizations`} first.`;
    const accountId = state.accountId ?? /arn:aws:[^:]*::(\d{12}):/.exec(outputs?.evalIdentityRef ?? '')?.[1] ?? null;
    return {
      plan: plans.get(input.sandboxId) ?? null,
      outputs,
      status: state.status,
      lastError: state.lastError,
      appliedAt: state.appliedAt,
      optimizations: state.optimizations,
      canApply: !busy && problems.length === 0,
      canDestroy: destroyBlockedReason === null,
      destroyBlockedReason,
      setupProblems: problems,
      account: { sandboxId: input.sandboxId, region: input.region, profile: settings.awsProfile || 'default', accountId, simulated },
      limits: { allowedInstanceTypes: input.allowedInstanceTypes, maxInstancesPerAgent: input.maxInstancesPerAgent, maxInstanceLifetimeHours: input.maxInstanceLifetimeHours },
    };
  }

  const requireSetup = (): void => {
    const problems = setupProblems();
    if (problems.length > 0) throw new Error(problems.join(' '));
  };

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
    async getOptimization(id) {
      return buildDetail(ledger, id, meta, now());
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
    async preflight({ agents, configYaml }) {
      return preflight(agents, configYaml);
    },
    async sandboxInfo(): Promise<SandboxInfo> {
      return buildSandboxInfo();
    },
    async planSandbox() {
      requireSetup();
      const input = sandboxInput(instanceTypes());
      plans.set(input.sandboxId, await app.engine.planSandbox(input));
      return buildSandboxInfo();
    },
    async applySandbox() {
      requireSetup();
      const input = sandboxInput(instanceTypes());
      await app.engine.applySandbox(input);
      plans.delete(input.sandboxId);
      rebuildLiveEngine(); // the live engine needs the new outputs
      return buildSandboxInfo();
    },
    async destroySandbox() {
      const id = currentSandboxId();
      await app.engine.destroySandbox(id);
      plans.delete(id);
      rebuildLiveEngine();
      return buildSandboxInfo();
    },
    async getSettings() {
      return settingsVM();
    },

    async createOptimization(input: CreateOptimizationInput) {
      if (input.configYaml !== undefined) {
        const v = validateConfigText(input.configYaml);
        if (!v.ok) throw new Error(`invalid ratchet.yaml: ${v.errors.map((e) => (e.path ? `${e.path}: ${e.message}` : e.message)).join('; ')}`);
      }
      if (settings.mode === 'live') assertSameRepo(input.repoUrl, settings.live.githubRepo);
      return app.engine.createOptimization({ ...input, sandboxId: currentSandboxId() });
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
    async teardown(id) {
      await app.engine.teardown(id);
      settings.markTornDown(id);
    },
    async exportReport(id): Promise<ExportReportResult> {
      const md = buildReport(ledger, id, meta, now());
      const name = `ratchet-report-${id}.md`;
      const path = deps.saveReport ? await deps.saveReport(name, md) : null;
      return path ? { saved: true, path } : { saved: false };
    },

    async setMode(mode) {
      if (mode !== settings.mode) {
        settings.setMode(mode);
        rebuild();
      }
      return settingsVM();
    },
    async setAwsProfile(profile) {
      settings.setAwsProfile(profile);
      rebuildLiveEngine();
      return settingsVM();
    },
    async setSecret(name: SecretName, value) {
      const problem = validateSecretFormat(name, value);
      if (problem) throw new Error(problem);
      settings.setSecret(name, value);
      rebuildLiveEngine();
      return settingsVM();
    },
    async clearSecret(name: SecretName) {
      settings.clearSecret(name);
      rebuildLiveEngine();
      return settingsVM();
    },
    async saveLiveSettings(input: LiveSettingsInput) {
      settings.setLive(input); // validates the whole batch first
      rebuildLiveEngine();
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
      unsubscribe();
      app.dispose();
      listeners.clear();
    },
  };

  async function preflight(agents: AgentKind[], configYaml: string): Promise<PreflightItem[]> {
    const live = settings.mode === 'live';
    const miss = (ok: boolean, needed: boolean): PreflightItem['state'] => (ok ? 'ok' : needed ? 'fail' : 'warn');
    const cfg = validateConfigText(configYaml);
    const items: PreflightItem[] = [
      {
        id: 'config',
        label: 'ratchet.yaml is valid',
        state: cfg.ok ? 'ok' : 'fail',
        detail: cfg.ok ? `${cfg.summary.metric} (${cfg.summary.direction})` : `${cfg.errors.length} error${cfg.errors.length === 1 ? '' : 's'}`,
      },
      { id: 'agents', label: 'At least one agent selected', state: agents.length > 0 ? 'ok' : 'fail', detail: agents.join(', ') || 'none' },
      {
        id: 'engine',
        label: live ? 'Live mode' : 'Simulation mode',
        state: app.kind === 'dev' ? 'warn' : 'ok',
        detail: app.kind === 'dev' ? 'dev engine stub: results are simulated' : live ? 'real AWS resources will be created' : 'no cloud resources are used',
      },
    ];
    if (agents.includes('claude-code')) {
      const ok = settings.hasSecret('anthropicApiKey');
      items.push({ id: 'anthropic', label: 'Anthropic API key', state: miss(ok, live), detail: ok ? 'configured' : live ? 'required in live mode (Settings)' : 'not needed in simulation' });
    }
    if (agents.includes('codex')) {
      const ok = settings.hasSecret('openaiApiKey');
      items.push({ id: 'openai', label: 'OpenAI API key', state: miss(ok, live), detail: ok ? 'configured' : live ? 'required in live mode (Settings)' : 'not needed in simulation' });
    }
    {
      const ok = settings.hasSecret('githubToken');
      items.push({ id: 'github', label: 'GitHub token', state: miss(ok, live), detail: ok ? 'configured' : live ? 'required to push branches and open the PR' : 'not needed in simulation' });
    }
    {
      const ok = settings.awsProfile !== '';
      items.push({ id: 'aws', label: 'AWS profile', state: miss(ok, live), detail: ok ? settings.awsProfile : live ? 'required in live mode (Settings)' : 'not needed in simulation' });
    }
    if (live) {
      const repoOk = settings.live.githubRepo !== '';
      items.push({ id: 'repo', label: 'GitHub repository', state: repoOk ? 'ok' : 'fail', detail: repoOk ? settings.live.githubRepo : 'required in live mode (Settings)' });
      const imagesOk = settings.live.runnerImageRef !== '' && settings.live.benchImageRef !== '';
      items.push({ id: 'images', label: 'Runner and bench AMIs', state: imagesOk ? 'ok' : 'fail', detail: imagesOk ? 'configured' : 'required in live mode (Settings)' });
    }
    const sandbox = await app.engine.sandboxState(currentSandboxId());
    const applied = sandbox.status === 'applied' && sandbox.outputs;
    items.push({
      id: 'sandbox',
      label: 'Sandbox applied',
      state: applied ? 'ok' : live ? 'fail' : 'warn',
      detail: applied ? sandbox.outputs!.artifactsBucket : live ? 'required in live mode: apply it under Environments' : 'not applied yet; apply it under Environments',
    });
    return items;
  }

  async function testConnection(target: ConnectionTarget): Promise<ConnectionResultVM> {
    const t = deps.connectionTester;
    const l = settings.live;
    const paths = effectivePaths(l, liveDefaults);
    const result = ((): Promise<ConnectionResultVM> => {
      if (!t) return Promise.resolve({ target, ok: false, detail: 'Connection tests are not available in this build.' });
      const need = (what: string): Promise<ConnectionResultVM> => Promise.resolve({ target, ok: false, detail: what });
      switch (target) {
        case 'aws': {
          const state = ledger.getSandbox(sandboxIdFor('live'));
          return t.aws({ region: l.awsRegion || 'us-east-1', profile: l.awsProfile || undefined, expectedAccountId: state?.status === 'applied' ? (state.accountId ?? undefined) : undefined });
        }
        case 'github': {
          const token = settings.getSecret('githubToken');
          if (!l.githubRepo) return need('Set the GitHub repository (owner/name) first.');
          return token ? t.github({ repo: l.githubRepo, token }) : need('Store a GitHub token first.');
        }
        case 'anthropic': {
          const apiKey = settings.getSecret('anthropicApiKey');
          return apiKey ? t.anthropic({ apiKey }) : need('Store an Anthropic API key first.');
        }
        case 'openai': {
          const apiKey = settings.getSecret('openaiApiKey');
          return apiKey ? t.openai({ apiKey }) : need('Store an OpenAI API key first.');
        }
        case 'tofu':
          return t.tofu({ binary: paths.tofuBinary, moduleDir: paths.moduleDir });
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
      const v = settings.getSecret(name);
      if (v && v.length >= 4) detail = detail.split(v).join('***');
    }
    if (target === 'aws' && !r.ok && l.awsAuth === 'sso' && l.awsProfile) detail += ` Run "aws sso login --profile ${l.awsProfile}" in a terminal, then test again.`;
    return { target, ok: r.ok, detail };
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

/** `acme/api`, `https://github.com/acme/api.git` and `git@github.com:acme/api` all name the same repository. */
function repoSlug(url: string): string {
  return (/(?:github\.com[/:])?([\w.-]+\/[\w.-]+?)(?:\.git)?\/?$/.exec(url.trim())?.[1] ?? url.trim()).toLowerCase();
}

/** Live mode runs on the one repository in Settings (its token, its clone); a different one in the form would silently be ignored. */
function assertSameRepo(formRepo: string, settingsRepo: string): void {
  if (settingsRepo === '') throw new Error('Set the GitHub repository in Settings before starting a live optimization.');
  if (repoSlug(formRepo) !== repoSlug(settingsRepo)) {
    throw new Error(`The repository ${formRepo} differs from the one in Settings (${settingsRepo}). Live optimizations run on the Settings repository.`);
  }
}
