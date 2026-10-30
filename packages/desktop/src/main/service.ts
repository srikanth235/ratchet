import type { EngineEvent, Ledger, SandboxInput } from '@ratchet/core';
import type { AppEngine } from './engine.js';
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
  CreateOptimizationInput,
  EngineEventVM,
  ExportReportResult,
  Mode,
  OptimizationsSnapshot,
  PreflightItem,
  RatchetApi,
  SandboxInfo,
  SecretName,
  SettingsVM,
} from '../shared/types.js';

export const SANDBOX_ID = 'ratchet-sandbox';
const DEFAULT_INSTANCE_TYPES = ['c7i.2xlarge', 'c7i.xlarge', 'm7i.large'];

export interface ServiceDeps {
  ledger: Ledger;
  settings: SettingsStore;
  /** Builds the engine for a mode; called at startup and on every mode switch. */
  createApp: (mode: Mode) => AppEngine;
  now?: () => number;
  /** Ask the user where to save the report; resolves to the written path, or null if cancelled. */
  saveReport?: (defaultName: string, markdown: string) => Promise<string | null>;
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

  const meta: MetaView = { prUrl: (id) => settings.prUrl(id), teardownDone: (id) => settings.tornDown(id) };

  function wire(a: AppEngine): () => void {
    return a.engine.subscribe((e) => {
      const vm = toEventVM(e);
      for (const l of listeners) l(vm);
    });
  }

  const engineInfo = () => ({ kind: app.kind, mode: app.engine.mode() });

  const settingsVM = (): SettingsVM => ({
    mode: settings.mode,
    awsProfile: settings.awsProfile,
    secrets: {
      anthropicApiKey: settings.hasSecret('anthropicApiKey'),
      openaiApiKey: settings.hasSecret('openaiApiKey'),
      githubToken: settings.hasSecret('githubToken'),
    },
    encryptionAvailable: settings.encryptionAvailable,
    engine: engineInfo(),
  });

  const sandboxInput = (types: string[] = DEFAULT_INSTANCE_TYPES): SandboxInput => ({
    sandboxId: SANDBOX_ID,
    region: 'us-east-1',
    allowedInstanceTypes: types,
    maxInstancesPerAgent: 2,
    maxInstanceLifetimeHours: 4,
    tags: { 'ratchet:managed': 'true' },
  });

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
      const latest = ledger.listOptimizations()[0];
      const cfg = latest ? validateConfigText(ledger.getOptimization(latest.id)!.configYaml) : null;
      const types = cfg?.ok ? cfg.summary.instanceTypes : DEFAULT_INSTANCE_TYPES;
      const input = sandboxInput(types);
      const [plan, outputs] = await Promise.all([app.sandbox.plan(input), app.sandbox.outputs(SANDBOX_ID)]);
      const accountId = /arn:aws:[^:]*::(\d{12}):/.exec(outputs?.evalIdentityRef ?? '')?.[1] ?? null;
      return {
        plan,
        outputs,
        account: { sandboxId: SANDBOX_ID, region: input.region, profile: settings.awsProfile || 'default', accountId, simulated: app.kind === 'dev' || settings.mode === 'simulation' },
        limits: { allowedInstanceTypes: input.allowedInstanceTypes, maxInstancesPerAgent: input.maxInstancesPerAgent, maxInstanceLifetimeHours: input.maxInstanceLifetimeHours },
      };
    },
    async getSettings() {
      return settingsVM();
    },

    async createOptimization(input: CreateOptimizationInput) {
      if (input.configYaml !== undefined) {
        const v = validateConfigText(input.configYaml);
        if (!v.ok) throw new Error(`invalid ratchet.yaml: ${v.errors.map((e) => (e.path ? `${e.path}: ${e.message}` : e.message)).join('; ')}`);
      }
      return app.engine.createOptimization(input);
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
        unsubscribe();
        app.dispose();
        app = deps.createApp(mode);
        unsubscribe = wire(app);
      }
      return settingsVM();
    },
    async setAwsProfile(profile) {
      settings.setAwsProfile(profile);
      return settingsVM();
    },
    async setSecret(name: SecretName, value) {
      settings.setSecret(name, value);
      return settingsVM();
    },
    async clearSecret(name: SecretName) {
      settings.clearSecret(name);
      return settingsVM();
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
    const outputs = await app.sandbox.outputs(SANDBOX_ID);
    items.push({ id: 'sandbox', label: 'Sandbox applied', state: outputs ? 'ok' : 'warn', detail: outputs ? outputs.artifactsBucket : 'not applied yet; review the plan under AWS sandbox' });
    return items;
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
