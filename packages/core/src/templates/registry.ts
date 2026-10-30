import { TEMPLATE_FILES } from './files.generated.js';

/**
 * Evaluator template registry: metadata and file lists only (no config schema, so `config/schema.ts` can import it).
 *
 * A template = a `ratchet.yaml` fragment + the evaluator's files + a `RATCHET.md` brief (see `renderTemplate` in
 * ./render.ts). Two kinds exist:
 *  - `template`: real, runnable evaluator files. `evaluator.template: <id>` in ratchet.yaml resolves to its command.
 *  - `demo`: "Try a demo" entries that only point at a ready-made project under `examples/`; they run in simulation
 *    with no setup, have no files of their own and cannot be used as `evaluator.template`.
 * "Packs" are not a user-facing concept any more: they are the template `category`.
 */

export type TemplateKind = 'template' | 'demo';
/** Gallery grouping (formerly "domain packs"; also the informational `pack:` of a generated ratchet.yaml). */
export type TemplateCategory = 'perf' | 'forecasting' | 'llm';
export type TemplateDirection = 'minimize' | 'maximize';
export type TemplateEvaluatorKind = 'benchmark' | 'backtest' | 'eval' | 'simulation';

/** Public, UI-friendly description of one template. Stable shape: the desktop gallery renders it. */
export interface TemplateInfo {
  /** Id used by `evaluator.template`, `ratchet init --template` and `proposeTemplate`. */
  id: string;
  kind: TemplateKind;
  category: TemplateCategory;
  title: string;
  /** One sentence: what improving this means. */
  description: string;
  /** What the evaluator reports, in plain words (e.g. "k6 p50/p95/p99, error rate"). */
  measures: string;
  /** The hidden workload that re-checks every win, or null when the template has none. */
  holdout: string | null;
  /** Default bench machine (instance type). */
  machine: string;
  evaluatorKind: TemplateEvaluatorKind;
  /** The default objective. Keys the evaluator prints, as a dotted path. */
  objective: { metric: string; direction: TemplateDirection; unit: string };
  /** Placeholder USD per unit of improvement per year, written to the generated config (replace it). */
  defaultValueUsd: number;
  /** Repo paths the template adds (empty for demos). */
  files: string[];
  /** Demos only: the example project, relative to the repository root (e.g. `examples/demo-search`). */
  example?: string;
}

/** What `evaluator.template` resolves to inside the config schema. */
export interface EvaluatorTemplate {
  id: string;
  run: string;
  evaluatorKind: TemplateEvaluatorKind;
  machine: string;
  /** Default guardrails, in ratchet.yaml form (`{ run: "..." }` / `{ metric: "< 0.001" }`), used when `guardrails:` is absent. */
  guardrails: Array<Record<string, string>>;
  /** Globs the template's evaluator needs protected (its files and data). */
  protect: string[];
}

interface TemplateDef extends Omit<TemplateInfo, 'files'> {
  run?: string;
  guardrails?: Array<Record<string, string>>;
  protect?: string[];
  /** Command guardrail written to the generated ratchet.yaml as a TODO (the repo's own test command). */
  testCommand?: boolean;
}

const DEFS: readonly TemplateDef[] = [
  {
    id: 'http-latency',
    kind: 'template',
    category: 'perf',
    title: 'HTTP latency',
    description: 'make a web service answer faster under a fixed request mix',
    measures: 'p50 / p95 / p99 latency of an open-loop load test, error rate',
    holdout: 'hidden request mix',
    machine: 'c7i.2xlarge',
    evaluatorKind: 'benchmark',
    objective: { metric: 'http_req_duration.p95', direction: 'minimize', unit: 'ms' },
    defaultValueUsd: 1000,
    run: 'node bench/http-latency.mjs',
    guardrails: [{ error_rate: '< 0.001' }],
    protect: ['bench/**'],
    testCommand: true,
  },
  {
    id: 'cli-cold-start',
    kind: 'template',
    category: 'perf',
    title: 'CLI cold start',
    description: 'make a command-line tool start and finish faster',
    measures: 'wall time of a set of invocations (hyperfine when installed), error rate',
    holdout: 'hidden invocations',
    machine: 'c7i.large',
    evaluatorKind: 'benchmark',
    objective: { metric: 'cold_start_ms.p50', direction: 'minimize', unit: 'ms' },
    defaultValueUsd: 400,
    run: 'node bench/cold-start.mjs',
    guardrails: [{ error_rate: '< 0.001' }],
    protect: ['bench/**'],
    testCommand: true,
  },
  {
    id: 'test-suite-time',
    kind: 'template',
    category: 'perf',
    title: 'Test suite time',
    description: 'make the test suite run faster without losing tests',
    measures: 'duration of the test command, whether it passed',
    holdout: null,
    machine: 'c7i.2xlarge',
    evaluatorKind: 'benchmark',
    objective: { metric: 'test_suite_ms', direction: 'minimize', unit: 'ms' },
    defaultValueUsd: 50,
    run: 'node bench/test-time.mjs',
    guardrails: [{ error_rate: '< 0.001' }],
    protect: ['bench/**'],
  },
  {
    id: 'build-size',
    kind: 'template',
    category: 'perf',
    title: 'Build time and bundle size',
    description: 'make the build faster or the shipped bundle smaller',
    measures: 'build seconds, bundle KB (raw and gzip)',
    holdout: null,
    machine: 'c7i.xlarge',
    evaluatorKind: 'benchmark',
    objective: { metric: 'bundle_kb', direction: 'minimize', unit: 'KB' },
    defaultValueUsd: 200,
    run: 'node bench/build-size.mjs',
    guardrails: [{ error_rate: '< 0.001' }],
    protect: ['bench/**'],
    testCommand: true,
  },
  {
    id: 'memory',
    kind: 'template',
    category: 'perf',
    title: 'Peak memory',
    description: 'make a workload use less memory',
    measures: 'peak resident set size (RSS) of the workload process tree',
    holdout: 'hidden workload',
    machine: 'c7i.xlarge',
    evaluatorKind: 'benchmark',
    objective: { metric: 'peak_rss_mb', direction: 'minimize', unit: 'MB' },
    defaultValueUsd: 300,
    run: 'node bench/memory.mjs',
    guardrails: [{ error_rate: '< 0.001' }],
    protect: ['bench/**'],
    testCommand: true,
  },
  {
    id: 'forecast-backtest',
    kind: 'template',
    category: 'forecasting',
    title: 'Forecast accuracy',
    description: 'make a demand, traffic or revenue forecast more accurate',
    measures: 'WAPE / MAPE / bias of a rolling-origin backtest, look-ahead findings',
    holdout: 'most recent periods',
    machine: 'c7i.xlarge',
    evaluatorKind: 'backtest',
    objective: { metric: 'wape', direction: 'minimize', unit: 'WAPE point' },
    defaultValueUsd: 10000,
    run: 'node eval/backtest.mjs',
    guardrails: [{ leakage_violations: '== 0' }],
    protect: ['eval/**', 'data/**'],
    testCommand: true,
  },
  {
    id: 'llm-eval',
    kind: 'template',
    category: 'llm',
    title: 'LLM eval score',
    description: 'make an LLM-backed task score higher, or cost less per call',
    measures: 'score on an eval set, cost and tokens per call, error rate',
    holdout: 'hidden eval set',
    machine: 'c7i.large',
    evaluatorKind: 'eval',
    objective: { metric: 'score', direction: 'maximize', unit: 'score point' },
    defaultValueUsd: 5000,
    run: 'node eval/llm-eval.mjs',
    guardrails: [{ error_rate: '< 0.05' }],
    protect: ['eval/**'],
    testCommand: true,
  },
  {
    id: 'demo-search',
    kind: 'demo',
    category: 'perf',
    title: 'Demo: search API latency',
    description: 'a search service whose p95 latency agents bring down (runs in simulation, no setup)',
    measures: 'k6 p50 / p95 / p99, error rate',
    holdout: 'hidden query mix',
    machine: 'c7i.2xlarge',
    evaluatorKind: 'benchmark',
    objective: { metric: 'http_req_duration.p95', direction: 'minimize', unit: 'ms' },
    defaultValueUsd: 1200,
    example: 'examples/demo-search',
  },
  {
    id: 'demo-cli',
    kind: 'demo',
    category: 'perf',
    title: 'Demo: CLI cold start',
    description: 'a task-manager CLI whose cold start agents shave (runs in simulation, no setup)',
    measures: 'hyperfine wall time per invocation',
    holdout: 'hidden invocations',
    machine: 'c7i.large',
    evaluatorKind: 'benchmark',
    objective: { metric: 'cold_start_ms.p50', direction: 'minimize', unit: 'ms' },
    defaultValueUsd: 400,
    example: 'examples/demo-cli',
  },
  {
    id: 'demo-forecast',
    kind: 'demo',
    category: 'forecasting',
    title: 'Demo: SKU demand forecast',
    description: 'a weekly demand forecaster whose WAPE agents lower (runs in simulation, no setup)',
    measures: 'WAPE / MAPE / bias of a rolling-origin backtest',
    holdout: 'the 12 weeks after the training data',
    machine: 'c7i.large',
    evaluatorKind: 'backtest',
    objective: { metric: 'wape', direction: 'minimize', unit: 'WAPE point' },
    defaultValueUsd: 20000,
    example: 'examples/demo-forecast',
  },
];

/** Legacy `ratchet init --pack` names, mapped to the template that replaced them. */
export const TEMPLATE_ALIASES: Readonly<Record<string, string>> = {
  perf: 'cli-cold-start',
  forecasting: 'forecast-backtest',
};

/** Resolve a template id or a legacy pack alias to a canonical template id (unchanged when unknown). */
export function resolveTemplateId(idOrAlias: string): string {
  return TEMPLATE_ALIASES[idOrAlias] ?? idOrAlias;
}

/** @internal definitions with their private fields; `listTemplates` is the public view. */
export function templateDefs(): readonly TemplateDef[] {
  return DEFS;
}

function infoOf(d: TemplateDef): TemplateInfo {
  const { run: _run, guardrails: _g, protect: _p, testCommand: _t, ...info } = d;
  return { ...info, objective: { ...d.objective }, files: d.kind === 'template' ? ['ratchet.yaml', ...Object.keys(TEMPLATE_FILES[d.id] ?? {})] : [] };
}

/**
 * Every template for the gallery: the runnable ones first (in table order), then the "Try a demo" entries.
 * Pure data; safe to call from the desktop main process or the CLI.
 */
export function listTemplates(opts: { kind?: TemplateKind; category?: TemplateCategory } = {}): TemplateInfo[] {
  return DEFS.filter((d) => (!opts.kind || d.kind === opts.kind) && (!opts.category || d.category === opts.category)).map(infoOf);
}

/** One template by id (legacy pack aliases accepted). */
export function getTemplate(idOrAlias: string): TemplateInfo | undefined {
  const id = resolveTemplateId(idOrAlias);
  const d = DEFS.find((x) => x.id === id);
  return d ? infoOf(d) : undefined;
}

/** Ids usable as `evaluator.template` (the runnable templates, not the demos). */
export function evaluatorTemplateIds(): string[] {
  return DEFS.filter((d) => d.kind === 'template').map((d) => d.id);
}

/** Resolve `evaluator.template`. Demos and unknown ids give undefined. */
export function findEvaluatorTemplate(id: string): EvaluatorTemplate | undefined {
  const d = DEFS.find((x) => x.id === id && x.kind === 'template');
  if (!d?.run) return undefined;
  return {
    id: d.id,
    run: d.run,
    evaluatorKind: d.evaluatorKind,
    machine: d.machine,
    guardrails: (d.guardrails ?? []).map((g) => ({ ...g })),
    protect: [...(d.protect ?? [])],
  };
}
