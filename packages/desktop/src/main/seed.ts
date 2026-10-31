import { parseConfig, type AgentKind, type CandidateVerdict, type Direction, type Ledger, type SpendCategory } from '@ratchet/core';
import { parseDocument } from 'yaml';
import { DEFAULT_CONFIG_YAML } from './default-config.js';
import { configColumns } from './dev-engine.js';
import { simulatedStats } from './dev-stats.js';

const MIN = 60_000;

interface CandSpec {
  /** Minutes after the optimization started. */
  t: number;
  agent: number; // index into spec.agents
  value: number;
  verdict: CandidateVerdict;
  message: string;
  reason: string;
  /** Hidden-trace value (verified / failed_confirm only). */
  confirm?: number;
}

interface OptSpec {
  id: string;
  name: string;
  repo: string;
  metric: string;
  direction: Direction;
  minGain: number;
  baseline: number;
  noise: number;
  agents: AgentKind[];
  status: 'running' | 'completed' | 'stopped' | 'draft';
  /** Minutes before `now` that the optimization started / ended. */
  startedAgo: number;
  endedAgo?: number;
  spendUsd: number;
  budgetUsd: number;
  /** Keep the demo config's `objective.value` (USD per ms of p95): only meaningful for the search demo. */
  valued?: boolean;
  /** A value model of its own: USD per metric unit per year. */
  value?: { usdPerUnit: number; unit: string };
  candidates: CandSpec[];
}

const around = (v: number): number[] => [0, 0.004, -0.003, 0.002, -0.005].map((d) => Number((v * (1 + d)).toFixed(3)));

const SPECS: OptSpec[] = [
  {
    id: 'opt_demo_search',
    name: 'search-api p95 latency',
    repo: 'acme/search-api',
    metric: 'http_req_duration.p95',
    direction: 'minimize',
    minGain: 0.03,
    baseline: 182,
    noise: 3.1,
    agents: ['claude-code', 'codex'],
    status: 'running',
    startedAgo: 224,
    spendUsd: 42.6,
    budgetUsd: 120,
    valued: true,
    // Verified wins carry the names of the real demo patches: the dev engine's diffs are those patches (dev-git.ts).
    candidates: [
      { t: 14, agent: 1, value: 179.5, verdict: 'rejected', message: 'Inline hot-path helper in query parser', reason: 'improvement 2.5 does not exceed required gain 5.46 (max of noise floor 3.1 and min_gain 5.46)' },
      { t: 31, agent: 0, value: 168, verdict: 'verified', confirm: 169.4, message: 'Drop the JSON deep-clone; build hits only for the returned page', reason: 'bench improvement 14 exceeds required gain 5.46' },
      { t: 55, agent: 1, value: 171, verdict: 'rejected', message: 'Pool JSON encoder buffers', reason: 'candidate 171 is not better than current best 168 (minimize)' },
      { t: 78, agent: 1, value: 156, verdict: 'verified', confirm: 158.2, message: 'Memoize parseFilter for repeated filter strings', reason: 'bench improvement 12 exceeds required gain 5.04' },
      { t: 96, agent: 0, value: 148, verdict: 'failed_confirm', confirm: 160.1, message: 'Skip scoring for zero-weight terms', reason: 'confirm run 160.1 does not beat current best 156 on the hidden trace (minimize)' },
      { t: 112, agent: 0, value: 149, verdict: 'verified', confirm: 150.7, message: 'Bounded top-k selection with a precomputed collation rank', reason: 'bench improvement 7 exceeds required gain 4.68' },
      { t: 125, agent: 1, value: 142, verdict: 'blocked', message: 'Tighten bench warm-up window', reason: 'protected path changed: bench/k6/search.js (matches bench/**)' },
      { t: 140, agent: 0, value: 152, verdict: 'rejected', message: 'Pre-size result arrays', reason: 'candidate 152 is not better than current best 149 (minimize)' },
      { t: 171, agent: 1, value: 138, verdict: 'verified', confirm: 140.3, message: 'Category / brand / tag position lists for filter-only queries', reason: 'bench improvement 11 exceeds required gain 4.47' },
      { t: 190, agent: 0, value: 140, verdict: 'rejected', message: 'Typed arrays in the scoring loop', reason: 'candidate 140 is not better than current best 138 (minimize)' },
      { t: 214, agent: 0, value: 131, verdict: 'verified', confirm: 133.0, message: 'Binary-search the sorted vocabulary for prefix terms', reason: 'bench improvement 7 exceeds required gain 4.14' },
    ],
  },
  {
    id: 'opt_demo_ingest',
    name: 'Ingest throughput',
    repo: 'acme/ingest',
    metric: 'throughput.events_per_s',
    direction: 'maximize',
    minGain: 0.03,
    baseline: 41.2,
    noise: 0.9,
    agents: ['codex'],
    status: 'completed',
    startedAgo: 24 * 60 + 90,
    endedAgo: 120,
    spendUsd: 61.2,
    budgetUsd: 100,
    value: { usdPerUnit: 400, unit: 'k/s' },
    candidates: [
      { t: 90, agent: 0, value: 49.8, verdict: 'verified', confirm: 49.1, message: 'Batch writes into 4 KiB segments', reason: 'bench improvement 8.6 exceeds required gain 1.24' },
      { t: 170, agent: 0, value: 50.4, verdict: 'rejected', message: 'Larger socket buffers', reason: 'improvement 0.6 does not exceed required gain 1.49' },
      { t: 260, agent: 0, value: 58.9, verdict: 'verified', confirm: 57.6, message: 'Vectorise the parser inner loop', reason: 'bench improvement 9.1 exceeds required gain 1.49' },
    ],
  },
  {
    id: 'opt_demo_edge',
    name: 'Edge worker cold start',
    repo: 'acme/edge-worker',
    metric: 'cold_start.s',
    direction: 'minimize',
    minGain: 0.03,
    baseline: 2.41,
    noise: 0.05,
    agents: ['claude-code', 'codex'],
    status: 'completed',
    // Finished about six weeks ago: the 30d KPI window compares against it.
    startedAgo: 41 * 24 * 60,
    endedAgo: 40 * 24 * 60,
    spendUsd: 38.9,
    budgetUsd: 60,
    value: { usdPerUnit: 3000, unit: 's' },
    candidates: [{ t: 120, agent: 1, value: 1.62, verdict: 'verified', confirm: 1.66, message: 'Lazy-load the template engine', reason: 'bench improvement 0.79 exceeds required gain 0.072' }],
  },
  {
    id: 'opt_demo_web',
    name: 'Web render p99',
    repo: 'acme/web',
    metric: 'render.p99',
    direction: 'minimize',
    minGain: 0.03,
    baseline: 0,
    noise: 0,
    agents: [],
    status: 'draft',
    startedAgo: 72 * 60,
    spendUsd: 0,
    budgetUsd: 50,
    candidates: [],
  },
];

/** Whether the ledger already holds data (so seeding is skipped). */
export function isLedgerEmpty(ledger: Ledger): boolean {
  return ledger.listOptimizations().length === 0;
}

/** Id of the seeded optimization that keeps "running" (the dev engine resumes simulating it). */
export const DEMO_RUNNING_ID = 'opt_demo_search';

/**
 * Seed a realistic set of optimizations through the normal Ledger API: one live run (11 candidates, 5 verified
 * ratchet moves, two agents), two finished ones and a draft. Idempotent per ledger: call only when it is empty.
 */
export function seedDemo(ledger: Ledger, now: number = Date.now()): void {
  for (const spec of SPECS) seedOne(ledger, spec, now);
}

function seedOne(ledger: Ledger, spec: OptSpec, now: number): void {
  const started = now - spec.startedAgo * MIN;
  const doc = parseDocument(DEFAULT_CONFIG_YAML);
  doc.setIn(['objective', 'name'], spec.name);
  doc.setIn(['objective', 'metric'], spec.metric);
  doc.setIn(['objective', 'direction'], spec.direction);
  if (spec.value) doc.setIn(['objective', 'value'], doc.createNode({ usd_per_unit: spec.value.usdPerUnit, period: 'year', basis: 'estimated', unit: spec.value.unit }));
  else if (!spec.valued) doc.deleteIn(['objective', 'value']);
  const yaml = doc.toString();
  const cfg = parseConfig(yaml);
  if (!cfg.ok) throw new Error(`seed config for ${spec.id} is invalid`);
  ledger.createOptimization({
    id: spec.id,
    name: spec.name,
    repo: spec.repo,
    baseRef: 'main',
    metric: spec.metric,
    direction: spec.direction,
    minGain: spec.minGain,
    budgetUsd: spec.budgetUsd,
    timeLimitS: 8 * 3600,
    configYaml: yaml,
    status: 'draft',
    createdAt: started - 5 * MIN,
    ...configColumns(cfg.config),
  });
  if (spec.status === 'draft') return;

  ledger.setOptimizationStatus(spec.id, 'baselining', started);
  const samples = around(spec.baseline);
  ledger.recordBaseline({
    optimizationId: spec.id,
    samples,
    noiseFloor: spec.noise,
    stats: simulatedStats(spec.metric, spec.baseline),
    recordedAt: started + 2 * MIN,
  });
  ledger.setOptimizationStatus(spec.id, 'running', started + 3 * MIN);

  const agentIds = spec.agents.map((kind, i) =>
    ledger.addAgent({
      id: `${spec.id}_agt_${i}`,
      optimizationId: spec.id,
      kind,
      branch: `ratchet/${spec.id.slice(9)}/${kind}-${i + 1}`,
      status: 'running',
      runnerInstanceId: `i-0${spec.id.length}a${i}c9e1b7d`,
      createdAt: started,
    }).id,
  );
  for (const [i, id] of agentIds.entries()) {
    ledger.appendAgentEvent({ agentId: id, type: 'status', payload: { text: 'Agent runner online; reading RATCHET.md' }, at: started + (4 + i) * MIN });
  }

  // spend weights → scaled to the target total
  const parts: { cat: SpendCategory; w: number }[] = [
    { cat: 'tokens', w: 0.55 },
    { cat: 'bench_runner', w: 0.3 },
    { cat: 'agent_runner', w: 0.09 },
    { cat: 'experiment', w: 0.05 },
    { cat: 'storage_egress', w: 0.01 },
  ];
  const wSum = parts.reduce((a, p) => a + p.w, 0) * Math.max(1, spec.candidates.length);
  const scale = spec.spendUsd / wSum;

  let best = spec.baseline;
  for (const [n, c] of spec.candidates.entries()) {
    const agentId = agentIds[c.agent]!;
    const at = started + c.t * MIN;
    const sha = (Math.abs(hash(`${spec.id}${n}`)).toString(16) + 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678').slice(0, 40);
    ledger.appendAgentEvent({ agentId, type: 'message', payload: { text: c.message }, at: at - 4 * MIN });
    ledger.appendAgentEvent({ agentId, type: 'tool', payload: { text: 'edit src/search/index.ts' }, at: at - 3 * MIN });
    ledger.appendAgentEvent({ agentId, type: 'commit', payload: { text: `Ratchet-Candidate: ${sha.slice(0, 7)}`, data: { sha } }, at: at - 1 * MIN });
    const cand = ledger.submitCandidate({ optimizationId: spec.id, agentId, sha, message: c.message, submittedAt: at });
    const stat = (v: number) => simulatedStats(spec.metric, v);
    const blocked = c.verdict === 'blocked';
    ledger.recordBenchResult({
      candidateId: cand.id,
      phase: 'bench',
      samples: around(c.value),
      stats: stat(c.value),
      guardrails: blocked ? [{ name: 'protected paths', passed: false, detail: c.reason }] : [{ name: 'pnpm test:unit', passed: true }, { name: 'error_rate < 0.001', passed: true }],
      startedAt: at + 20_000,
      finishedAt: at + 4 * MIN,
    });
    if (c.confirm !== undefined) {
      ledger.recordBenchResult({ candidateId: cand.id, phase: 'confirm', samples: around(c.confirm), stats: stat(c.confirm), guardrails: [], startedAt: at + 4 * MIN, finishedAt: at + 7 * MIN });
    }
    ledger.setVerdict(cand.id, c.verdict, [c.reason], at + 7 * MIN);
    if (c.verdict === 'verified') {
      ledger.moveRatchet(spec.id, cand.id, c.value, at + 7 * MIN);
      best = c.value;
    }
    ledger.appendAgentEvent({ agentId, type: 'verdict', payload: { text: `${c.verdict}: ${c.reason}`, data: { sha, verdict: c.verdict, value: c.value, best } }, at: at + 7 * MIN });
    for (const p of parts) ledger.recordSpend({ optimizationId: spec.id, agentId, category: p.cat, usd: Number((p.w * scale).toFixed(4)), source: 'seed', at: at + 5 * MIN });
  }

  if (spec.status === 'completed') {
    for (const id of agentIds) ledger.setAgentStatus(id, 'done');
    ledger.setOptimizationStatus(spec.id, 'completed', now - (spec.endedAgo ?? 0) * MIN);
  }
}

function hash(s: string): number {
  let h = 0;
  for (let i = 0; i < s.length; i++) h = (Math.imul(31, h) + s.charCodeAt(i)) | 0;
  return h;
}
