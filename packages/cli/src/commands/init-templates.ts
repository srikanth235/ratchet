/**
 * Everything `ratchet init` writes, in one place: the pack catalog, the defaults of each pack, and the text of
 * ratchet.yaml, RATCHET.md and the evaluator scaffolding. Each pack renders a list of files for a set of options.
 */
export type Direction = 'minimize' | 'maximize';

export interface InitOptions {
  /** Key in the evaluator's JSON output that is the objective (dotted path). */
  metric: string;
  direction: Direction;
  /** USD per one unit of metric improvement, per year. */
  valueUsd: number;
}

export interface PackTemplate {
  id: string;
  title: string;
  description: string;
  /** Unit one metric point is measured in, for the value model. */
  unit: string;
  evaluatorKind: 'benchmark' | 'backtest';
  defaults: InitOptions;
  /** Files relative to the repo root; `executable: false` for all of them, kept simple. */
  render(o: InitOptions): TemplateFile[];
}

export interface TemplateFile {
  path: string;
  content: string;
}

/** A YAML scalar: plain when it is safe, JSON-quoted (valid YAML) otherwise. */
function scalar(s: string): string {
  return /^[A-Za-z_][\w.-]*$/.test(s) ? s : JSON.stringify(s);
}

/** Dotted path -> JSON key list, for the stub evaluators ("a.b" -> ["a","b"]). */
const pathLiteral = (metric: string) => JSON.stringify(metric.split('.'));

const PERF_YAML = (o: InitOptions): string => `version: 0
pack: perf                         # domain pack: code performance (see docs/packs/perf.md)
objective:
  name: ${scalar(o.metric)}
  metric: ${scalar(o.metric)}      # key in the benchmark's JSON output (dotted path)
  direction: ${o.direction}
  min_gain: 0.03                   # 3% relative: smaller changes are indistinguishable from noise
  value:                           # business case: what one unit of the metric is worth
    usd_per_unit: ${o.valueUsd}    # USD per ${perfUnit(o)} per year. A placeholder: replace it with the customer's own numbers
    period: year
    basis: estimated               # becomes \`measured\` once confirmed in production
    unit: ${perfUnit(o)}
evaluator:
  kind: benchmark
  ref: ratchet/bench               # protected ref holding the trusted bench
  run: node bench/run.mjs          # TODO: the real benchmark (k6, hyperfine --export-json, ...). It must print JSON on stdout
  repeats: 5                       # also used for the baseline noise floor
  hardware: c7i.large              # pinned, dedicated host
  # holdout: s3://sealed/trace.json  # TODO: a hidden, different workload the agents never see (the winner is re-checked on it)
guardrails:
  - run: npm test                  # TODO: your test command; must exit 0
  - error_rate: "< 0.001"          # reported by the benchmark
protected:                         # changes here block a candidate
  - bench/**
  - ratchet.yaml
  - package.json                   # defines the guardrail commands
agent:
  instructions: RATCHET.md         # brief injected into every session
  allowed_instance_types: [c7i.large, c7i.xlarge, m7i.large]
`;

const perfUnit = (o: InitOptions): string => (/(^|[._])ms\b|_ms|latency/.test(o.metric) ? 'ms' : 'unit');

const PERF_BRIEF = (o: InitOptions): string => `# Ratchet brief: ${o.direction === 'minimize' ? 'lower' : 'raise'} \`${o.metric}\`

**Goal:** ${o.direction} \`${o.metric}\`, reported by the benchmark in \`bench/run.mjs\`. A change counts only if a trusted
benchmark, which you cannot edit, confirms it, then again on a hidden workload you do not see.
Your own measurements are for guiding your work.

## Rules

- Do **not** modify protected paths: \`bench/**\`, \`ratchet.yaml\`, \`package.json\`. A candidate that touches them is blocked.
- Keep behaviour: the guardrails (\`npm test\`, error rate) must stay green.
- Don't special-case the benchmark's workload: the final check runs a different one of the same kind.
- Don't add dependencies unless a change clearly needs one.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  \`Ratchet-Candidate: true\` to the commit.

## Workflow

\`\`\`sh
npm test             # must pass before and after
node bench/run.mjs   # prints the benchmark JSON, including \`${o.metric}\`
\`\`\`

TODO: describe the system (where the time goes, how to profile it), and the traps you know about.
`;

const PERF_BENCH = (o: InitOptions): string => `#!/usr/bin/env node
// Benchmark stub (protected: bench/**). Replace COMMAND with the real workload, or point evaluator.run in ratchet.yaml at
// your own tool (k6, hyperfine --export-json, ...) as long as it prints JSON on stdout. Prints the wall-clock time of
// COMMAND over RUNS runs: { latency_ms: { p50, p95, mean, min, max }, error_rate, runs }.
// RATCHET_TRACE, when set, names the sealed holdout workload; the stub ignores it. TODO: load your hidden workload from it.
import { spawnSync } from 'node:child_process';

const COMMAND = process.env.BENCH_COMMAND ?? 'node --version'; // TODO: the command whose speed matters
const RUNS = Number(process.env.BENCH_RUNS ?? 20);
const METRIC = ${pathLiteral(o.metric)}; // objective.metric of ratchet.yaml, as a path into the JSON below

const times = [];
let failures = 0;
for (let i = 0; i < RUNS; i++) {
  const t0 = process.hrtime.bigint();
  const r = spawnSync(COMMAND, { shell: true, stdio: 'ignore' });
  times.push(Number(process.hrtime.bigint() - t0) / 1e6);
  if (r.status !== 0) failures++;
}
times.sort((a, b) => a - b);
const pick = (p) => times[Math.min(times.length - 1, Math.ceil(p * times.length) - 1)];
const round = (x) => Math.round(x * 1000) / 1000;
const latency = {
  p50: round(pick(0.5)),
  p95: round(pick(0.95)),
  mean: round(times.reduce((a, b) => a + b, 0) / times.length),
  min: round(times[0]),
  max: round(times.at(-1)),
};

const result = { latency_ms: latency, error_rate: failures / RUNS, runs: RUNS };
// A metric that is not one of the numbers above: compute it here (TODO) and store it where objective.metric points.
if (METRIC[0] !== 'latency_ms' && METRIC[0] !== 'error_rate' && METRIC[0] !== 'runs') {
  let node = result;
  for (const key of METRIC.slice(0, -1)) node = node[key] ??= {};
  node[METRIC.at(-1)] = latency.p95; // TODO
}
console.log(JSON.stringify(result));
`;

const FORECAST_YAML = (o: InitOptions): string => `version: 0
pack: forecasting                  # domain pack: forecasting (see docs/packs/forecasting.md)
objective:
  name: ${scalar(o.metric)}
  metric: ${scalar(o.metric)}      # key in the evaluator's JSON output (dotted path)
  direction: ${o.direction}
  min_gain: 0.03                   # 3% relative
  value:                           # business case: what one point of forecast error is worth
    usd_per_unit: ${o.valueUsd}    # USD per point of ${o.metric} per year. A placeholder: replace it with the customer's own numbers
    period: year
    basis: estimated               # becomes \`measured\` once confirmed in production
    unit: ${scalar(`${o.metric} point`)}
evaluator:
  kind: backtest
  ref: ratchet/bench               # protected ref holding the trusted evaluator
  run: node eval/backtest.mjs      # rolling-origin backtest -> { wape, bias, leakage_violations }; with RATCHET_TRACE set, scores the sealed holdout
  repeats: 3                       # an error metric is deterministic; repeats guard timing jitter
  hardware: c7i.large              # pinned, dedicated host
  # holdout: s3://sealed/holdout.json  # TODO: the most recent period, never shown to the agents (the winner is re-scored on it)
guardrails:
  - run: npm test                  # TODO: your test command; must exit 0
  - leakage_violations: "== 0"     # look-ahead findings, reported by the evaluator itself
protected:                         # changes here block a candidate
  - eval/**
  - data/**
  - ratchet.yaml
  - package.json                   # defines the guardrail commands
agent:
  instructions: RATCHET.md         # brief injected into every session
  allowed_instance_types: [c7i.large, c7i.xlarge, m7i.large]
`;

const FORECAST_BRIEF = (o: InitOptions): string => `# Ratchet brief: ${o.direction === 'minimize' ? 'lower' : 'raise'} \`${o.metric}\` of the forecast

**Goal:** ${o.direction} \`${o.metric}\`, reported by the rolling-origin backtest in \`eval/backtest.mjs\`: the forecast is made
at several past origins and scored on the weeks that followed. A change counts only if the trusted evaluator, which you
cannot edit, confirms it, then again on a sealed holdout (the most recent weeks), which you never see.

## Rules

- Do **not** modify protected paths: \`eval/**\`, \`data/**\`, \`ratchet.yaml\`, \`package.json\`. A candidate that touches them
  is blocked.
- **No look-ahead.** \`forecast(history, horizon)\` in \`src/forecast.mjs\` may use only the history it is handed. The evaluator
  reports \`leakage_violations\`, which must be 0.
- Don't memorize the backtest: constants fitted to particular origins are rejected on the holdout.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  \`Ratchet-Candidate: true\` to the commit.

## Workflow

\`\`\`sh
npm test               # must pass before and after
node eval/backtest.mjs # prints { wape, bias, leakage_violations }
\`\`\`

TODO: describe the data (granularity, seasonality, promotions, stockouts) and what you know drives the demand.
`;

const FORECAST_BACKTEST = (o: InitOptions): string => `#!/usr/bin/env node
// Rolling-origin backtest stub (protected: eval/**). For each origin the model sees only the history before it and
// forecasts HORIZON periods; the forecasts are scored on the periods that followed. Prints
// { wape, bias } (percent) and leakage_violations (a count). Replace loadSeries() with the real data (data/**).
// RATCHET_TRACE, when set, names the sealed holdout series file: [[period values of series 1], [series 2], ...].
import { readFileSync } from 'node:fs';
import { forecast } from '../src/forecast.mjs';

const HORIZON = 4;
const ORIGINS = [52, 60, 68, 76, 84];
const METRIC = ${pathLiteral(o.metric)}; // objective.metric of ratchet.yaml, as a path into the JSON below

/** TODO: the real series. This is a seeded synthetic one: weekly demand with a yearly season and a trend. */
function loadSeries() {
  if (process.env.RATCHET_TRACE) return JSON.parse(readFileSync(process.env.RATCHET_TRACE, 'utf8'));
  let seed = 7;
  const rng = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32);
  return Array.from({ length: 20 }, (_, s) =>
    Array.from({ length: 104 }, (_, t) => Math.max(0, Math.round(40 + s * 3 + t * 0.1 + 15 * Math.sin((2 * Math.PI * t) / 52) + (rng() - 0.5) * 12))),
  );
}

const series = loadSeries();
let absErr = 0, signedErr = 0, total = 0;
for (const origin of ORIGINS) {
  for (const actual of series) {
    const history = Object.freeze(actual.slice(0, origin)); // nothing at or after the origin is visible
    const predicted = forecast(history, HORIZON);
    if (predicted.length !== HORIZON || predicted.some((v) => !Number.isFinite(v) || v < 0)) throw new Error('forecast() must return HORIZON finite, non-negative numbers');
    for (let h = 0; h < HORIZON; h++) {
      const truth = actual[origin + h];
      if (truth === undefined) continue;
      absErr += Math.abs(predicted[h] - truth);
      signedErr += predicted[h] - truth;
      total += truth;
    }
  }
}

const round = (x) => Math.round(x * 1000) / 1000;
const result = { wape: round((100 * absErr) / total), bias: round((100 * signedErr) / total), leakage_violations: 0 };
// The model is only handed the history, so it cannot read the future. TODO: if your pipeline loads data itself, scramble
// the periods after each origin and count the origins where the forecast changes: that count is leakage_violations.
// A metric that is not one of the numbers above: compute it here (TODO) and store it where objective.metric points.
if (METRIC[0] !== 'wape' && METRIC[0] !== 'bias' && METRIC[0] !== 'leakage_violations') {
  let node = result;
  for (const key of METRIC.slice(0, -1)) node = node[key] ??= {};
  node[METRIC.at(-1)] = result.wape; // TODO
}
console.log(JSON.stringify(result));
`;

const FORECAST_MODEL = `// The model Ratchet optimizes. TODO: replace the baseline with your forecaster.
// \`history\` is the series up to the forecast origin (never later periods); return \`horizon\` finite, non-negative numbers.

/** Baseline: the mean of the last 4 periods, repeated. */
export function forecast(history, horizon) {
  const recent = history.slice(-4);
  const level = recent.reduce((a, b) => a + b, 0) / Math.max(1, recent.length);
  return Array.from({ length: horizon }, () => level);
}
`;

export const PACKS: readonly PackTemplate[] = [
  {
    id: 'perf',
    title: 'Code performance',
    description: 'make a service, CLI or library faster (latency, throughput, memory) against a trusted benchmark',
    unit: 'ms',
    evaluatorKind: 'benchmark',
    defaults: { metric: 'latency_ms.p95', direction: 'minimize', valueUsd: 100 },
    render: (o) => [
      { path: 'ratchet.yaml', content: PERF_YAML(o) },
      { path: 'RATCHET.md', content: PERF_BRIEF(o) },
      { path: 'bench/run.mjs', content: PERF_BENCH(o) },
    ],
  },
  {
    id: 'forecasting',
    title: 'Forecasting',
    description: 'make a demand/traffic/revenue forecast more accurate against a rolling-origin backtest',
    unit: 'WAPE point',
    evaluatorKind: 'backtest',
    defaults: { metric: 'wape', direction: 'minimize', valueUsd: 10000 },
    render: (o) => [
      { path: 'ratchet.yaml', content: FORECAST_YAML(o) },
      { path: 'RATCHET.md', content: FORECAST_BRIEF(o) },
      { path: 'eval/backtest.mjs', content: FORECAST_BACKTEST(o) },
      { path: 'src/forecast.mjs', content: FORECAST_MODEL },
    ],
  },
];

export function findPack(id: string): PackTemplate | undefined {
  return PACKS.find((p) => p.id === id);
}
