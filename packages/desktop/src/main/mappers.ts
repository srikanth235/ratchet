import {
  effectiveProtected,
  getTemplate,
  parseConfig,
  spendEvents,
  type FileDiff,
  type OptimizationRow,
  type ActivityItem as LedgerActivity,
  type AgentRow,
  type CandidateWithResults,
  type Ledger,
  type OptimizationSummary,
  type SandboxTeardownReceipt,
  type SpendBreakdown,
  type TemplateInfo,
  CALIBRATION,
} from '@ratchet/core';
import type {
  ActivityItem,
  AgentEventVM,
  AgentEventsPage,
  AgentVM,
  BenchPipelineVM,
  CandidateVM,
  ConfigSummary,
  DiffLine,
  Optimization,
  OptimizationDetail,
  OptimizationStatus,
  PipelineStage,
  RatchetPoint,
  RatchetSeries,
  SpendVM,
  TeardownReceiptVM,
  TemplateVM,
  ValidateConfigResult,
  WinningChangeVM,
} from '../shared/types.js';
import { median } from './kpis.js';
import { compareRows, metricOptions, spanRows, statOf, unitForMetric } from './metrics.js';
import { valueOf } from './value.js';

export { buildKpis, median } from './kpis.js';
export { unitForMetric } from './metrics.js';

/** State the ledger does not hold (kept in the settings store by the service). */
export interface MetaView {
  prUrl(optimizationId: string): string | null;
}

const MIN = 60_000;

// ------------------------------------------------------------------ small pure helpers

export function relTime(at: number, now: number): string {
  const d = Math.max(0, now - at);
  if (d < MIN) return 'just now';
  if (d < 60 * MIN) return `${Math.floor(d / MIN)}m ago`;
  if (d < 24 * 60 * MIN) return `${Math.floor(d / (60 * MIN))}h ago`;
  const days = Math.floor(d / (24 * 60 * MIN));
  return days === 1 ? 'yesterday' : `${days}d ago`;
}

export function toUiStatus(status: OptimizationSummary['status'], prUrl: string | null): OptimizationStatus {
  if (status === 'completed' && prUrl) return 'pr-open';
  return status;
}

export function uiDirection(d: 'minimize' | 'maximize'): 'lower' | 'higher' {
  return d === 'minimize' ? 'lower' : 'higher';
}

/** Evenly pick at most `n` values (always keeping the last). */
export function downsample(xs: readonly number[], n = 8): number[] {
  if (xs.length <= n) return [...xs];
  return Array.from({ length: n }, (_, i) => xs[Math.round((i * (xs.length - 1)) / (n - 1))]!);
}

const optName = (o: { name: string | null; repo: string; metric: string }): string => o.name ?? `${o.repo} ${o.metric}`;

// ------------------------------------------------------------------ optimizations

export function toOptimization(s: OptimizationSummary, row: OptimizationRow, meta: MetaView, now: number): Optimization {
  const live = s.status === 'running' || s.status === 'baselining';
  return {
    id: s.id,
    name: optName(s),
    repo: s.repo,
    metric: s.metric,
    unit: unitForMetric(s.metric),
    direction: uiDirection(s.direction),
    baseline: s.baselineValue,
    best: s.bestValue,
    gainPct: s.gainPct,
    status: toUiStatus(s.status, meta.prUrl(s.id)),
    agents: s.agentKinds,
    spendUsd: s.spendUsd,
    budgetUsd: s.budgetUsd,
    updated: live ? 'live' : relTime(s.endedAt ?? s.startedAt ?? s.createdAt, now),
    candidateCount: s.candidateCount,
    verifiedWins: s.verifiedWins,
    createdAt: s.createdAt,
    pack: row.pack,
    evaluatorKind: row.evaluatorKind,
    value: valueOf(row, s.baselineValue, s.bestValue, s.spendUsd),
  };
}

/**
 * The ratchet chart. For the objective: baseline step, one step per ratchet move, a point per benched candidate.
 * For another metric of the objective's family (`metric`): the same commits, plotted by that metric's median
 * (the "best" line follows the verified commits; there is no noise floor for it).
 */
export function buildSeries(ledger: Ledger, id: string, now: number, metric?: string): RatchetSeries {
  const opt = ledger.getOptimization(id);
  if (!opt) throw new Error(`optimization ${id} not found`);
  const t0 = opt.startedAt ?? opt.createdAt;
  const minutes = (at: number) => Math.max(0, (at - t0) / MIN);
  const baseline = ledger.getBaseline(id);
  const moves = ledger.ratchetHistory(id);
  const cands = ledger.candidatesWithResults(id);
  const options = baseline ? metricOptions(opt.metric, opt.configYaml, baseline.stats) : [];
  const key = metric && options.some((o) => o.key === metric) ? metric : opt.metric;
  const objective = key === opt.metric;
  const benchStats = (c: CandidateWithResults) => c.results.filter((r) => r.phase === 'bench').at(-1)?.stats;
  const valueOfCand = (c: CandidateWithResults) => (objective ? c.benchValue : statOf(benchStats(c), key, opt.metric));
  const byId = new Map(cands.map((c) => [c.id, c]));

  const steps: { t: number; value: number }[] = [];
  const base = baseline ? (objective ? baseline.value : statOf(baseline.stats, key, opt.metric)) : null;
  if (base !== null) steps.push({ t: 0, value: base });
  for (const m of moves) {
    const v = objective ? m.toValue : (() => {
      const c = byId.get(m.candidateId);
      return c ? valueOfCand(c) : null;
    })();
    if (v !== null) steps.push({ t: minutes(m.at), value: v });
  }

  const points: RatchetPoint[] = cands.flatMap((c) => {
    const v = valueOfCand(c);
    return v === null
      ? []
      : [{ t: minutes(c.submittedAt), value: v, agent: c.agentKind, verified: c.movedRatchet, label: `${c.sha.slice(0, 7)} · ${c.message || c.verdict}` }];
  });

  const last = Math.max(0, ...steps.map((s) => s.t), ...points.map((p) => p.t));
  const live = opt.status === 'running' || opt.status === 'baselining';
  return {
    metric: key,
    options: options.length ? options : [{ key: opt.metric, label: opt.metric, unit: unitForMetric(opt.metric), objective: true }],
    unit: unitForMetric(key),
    direction: uiDirection(opt.direction),
    baseline: base,
    steps,
    points,
    noiseFloor: objective ? baseline?.noiseFloor ?? null : null,
    tMax: live ? Math.max(last, minutes(now)) : last,
  };
}

// ------------------------------------------------------------------ candidates

export function toCandidateVM(c: CandidateWithResults, opt: { startedAt: number | null; createdAt: number }, baseline: number | null, direction: 'minimize' | 'maximize'): CandidateVM {
  const t0 = opt.startedAt ?? opt.createdAt;
  const bench = c.results.filter((r) => r.phase === 'bench').at(-1);
  let vs: number | null = null;
  if (baseline && c.benchValue !== null) {
    const delta = direction === 'minimize' ? baseline - c.benchValue : c.benchValue - baseline;
    vs = (delta / Math.abs(baseline)) * 100;
  }
  return {
    id: c.id,
    sha: c.sha.slice(0, 7),
    agentId: c.agentId,
    agent: c.agentKind,
    message: c.message,
    submittedAt: c.submittedAt,
    t: Math.max(0, (c.submittedAt - t0) / MIN),
    verdict: c.verdict,
    reasons: c.reasons,
    benchValue: c.benchValue,
    confirmValue: c.confirmValue,
    vsBaselinePct: vs,
    movedRatchet: c.movedRatchet,
    samples: bench?.samples ?? [],
    stats: bench?.stats ?? {},
  };
}

export function buildCandidates(ledger: Ledger, id: string): CandidateVM[] {
  const opt = ledger.getOptimization(id);
  if (!opt) throw new Error(`optimization ${id} not found`);
  const baseline = ledger.getBaseline(id)?.value ?? null;
  return ledger.candidatesWithResults(id).map((c) => toCandidateVM(c, opt, baseline, opt.direction));
}

// ------------------------------------------------------------------ spend

const CATEGORY_LABEL: Record<string, string> = {
  agent_runner: 'Agent runners',
  experiment: 'Experiment instances',
  bench_runner: 'Bench runner',
  storage_egress: 'Storage & egress',
  tokens: 'Model tokens',
};

export function toSpendVM(b: SpendBreakdown, budgetUsd: number, series: { t: number; usd: number }[]): SpendVM {
  return {
    total: b.total,
    estimated: b.estimated,
    actual: b.actual,
    budgetUsd,
    byCategory: Object.entries(b.byCategory)
      .map(([id, usd]) => ({ id, label: CATEGORY_LABEL[id] ?? id, usd }))
      .sort((a, b2) => b2.usd - a.usd),
    byAgent: b.byAgent.map((a) => ({ agentId: a.agentId, kind: a.kind, branch: a.branch, usd: a.total })),
    series,
  };
}

/** Cumulative spend, bucketed to at most `points` samples, in minutes since the optimization started. */
export function spendSeries(ledger: Ledger, id: string, points = 40): { t: number; usd: number }[] {
  const opt = ledger.getOptimization(id);
  if (!opt) return [];
  const t0 = opt.startedAt ?? opt.createdAt;
  const rows = (ledger.db.select({ at: spendEvents.at, usd: spendEvents.usd, opt: spendEvents.optimizationId }).from(spendEvents).all())
    .filter((r) => r.opt === id)
    .sort((a, b) => a.at - b.at);
  let cum = 0;
  const series = rows.map((r) => ({ t: Math.max(0, (r.at - t0) / MIN), usd: (cum += r.usd) }));
  if (series.length <= points) return series;
  return Array.from({ length: points }, (_, i) => series[Math.round((i * (series.length - 1)) / (points - 1))]!);
}

export function buildSpend(ledger: Ledger, id: string): SpendVM {
  const opt = ledger.getOptimization(id);
  if (!opt) throw new Error(`optimization ${id} not found`);
  return toSpendVM(ledger.spendBreakdown(id), opt.budgetUsd, spendSeries(ledger, id));
}

// ------------------------------------------------------------------ activity

export function toActivity(a: LedgerActivity, metricUnit: (optId: string) => string, now: number): ActivityItem {
  const short = a.sha.slice(0, 7);
  if (a.kind === 'ratchet_move') {
    const unit = metricUnit(a.optimizationId);
    return {
      id: `m-${a.candidateId}`,
      optimizationId: a.optimizationId,
      agent: a.agentKind,
      text: `Verified win: ${a.message || short} · ${fmtNum(a.fromValue)} → ${fmtNum(a.toValue)}${unit ? ` ${unit}` : ''}`,
      at: relTime(a.at, now),
      atMs: a.at,
      tone: 'mint',
    };
  }
  const why = a.reasons[0] ?? '';
  const text: Record<string, string> = {
    verified: `Candidate verified: ${a.message || short}`,
    rejected: `Candidate rejected: ${why || a.message || short}`,
    failed_confirm: `Failed confirm on the hidden trace: ${a.message || short}`,
    blocked: `Blocked: ${why || a.message || short}`,
  };
  return {
    id: `v-${a.candidateId}`,
    optimizationId: a.optimizationId,
    agent: a.agentKind,
    text: text[a.verdict] ?? `${a.verdict}: ${short}`,
    at: relTime(a.at, now),
    atMs: a.at,
    tone: a.verdict === 'verified' ? 'mint' : a.verdict === 'blocked' || a.verdict === 'failed_confirm' ? 'rose' : 'neutral',
  };
}

export function buildActivity(ledger: Ledger, now: number, limit = 12, optimizationId?: string): ActivityItem[] {
  const units = new Map(ledger.listOptimizations().map((o) => [o.id, unitForMetric(o.metric)]));
  const items = ledger.recentActivity(limit * 2, optimizationId ? { optimizationId } : {});
  // A verified verdict is shown through its ratchet move; drop the duplicate verdict row.
  const moved = new Set(items.filter((i) => i.kind === 'ratchet_move').map((i) => i.candidateId));
  return items
    .filter((i) => !(i.kind === 'verdict' && i.verdict === 'verified' && moved.has(i.candidateId)))
    .slice(0, limit)
    .map((i) => toActivity(i, (id) => units.get(id) ?? '', now));
}

function fmtNum(n: number): string {
  return Number.isInteger(n) ? String(n) : String(Number(n.toFixed(2)));
}

// ------------------------------------------------------------------ agents & events

interface EventRow {
  id: number;
  agent_id: string;
  at: number;
  type: string;
  payload: string | null;
}

function parsePayload(raw: string | null): { text: string; from?: string } {
  if (!raw) return { text: '' };
  try {
    const p: unknown = JSON.parse(raw);
    if (p && typeof p === 'object') {
      const o = p as { text?: unknown; data?: { from?: unknown } };
      const out: { text: string; from?: string } = { text: typeof o.text === 'string' ? o.text : '' };
      if (typeof o.data?.from === 'string') out.from = o.data.from;
      return out;
    }
    return { text: String(p) };
  } catch {
    return { text: raw };
  }
}

export function toEventVM(r: EventRow): AgentEventVM {
  return { id: r.id, at: r.at, type: r.type, text: parsePayload(r.payload).text };
}

/**
 * Agent log page. No cursor → the newest `limit` events (oldest first); with a cursor → events newer than it.
 * Raw SQL because `Ledger.agentEvents` has no "tail" mode (proposed for core).
 */
export function agentEventsPage(ledger: Ledger, agentId: string, cursor: number | undefined, limit = 200): AgentEventsPage {
  if (!ledger.getAgent(agentId)) throw new Error(`agent ${agentId} not found`);
  const rows =
    cursor === undefined
      ? (ledger.sqlite
          .prepare('SELECT * FROM (SELECT id, agent_id, at, type, payload FROM agent_events WHERE agent_id = ? ORDER BY id DESC LIMIT ?) ORDER BY id ASC')
          .all(agentId, limit) as EventRow[])
      : (ledger.sqlite
          .prepare('SELECT id, agent_id, at, type, payload FROM agent_events WHERE agent_id = ? AND id > ? ORDER BY id ASC LIMIT ?')
          .all(agentId, cursor, limit) as EventRow[]);
  return { events: rows.map(toEventVM), cursor: rows.at(-1)?.id ?? cursor ?? 0 };
}

function latestAgentMessage(ledger: Ledger, agentId: string): string | null {
  const rows = ledger.sqlite
    .prepare("SELECT id, agent_id, at, type, payload FROM agent_events WHERE agent_id = ? AND type = 'message' ORDER BY id DESC LIMIT 20")
    .all(agentId) as EventRow[];
  for (const r of rows) {
    const p = parsePayload(r.payload);
    if (p.from !== 'user' && p.text) return p.text;
  }
  return null;
}

function lastEventId(ledger: Ledger, agentId: string): number {
  const r = ledger.sqlite.prepare('SELECT max(id) AS m FROM agent_events WHERE agent_id = ?').get(agentId) as { m: number | null };
  return r.m ?? 0;
}

export function toAgentVM(ledger: Ledger, a: AgentRow, cands: CandidateWithResults[], spendUsd: number): AgentVM {
  const mine = cands.filter((c) => c.agentId === a.id);
  const recent = mine
    .filter((c) => c.benchValue !== null)
    .slice(-10)
    .map((c) => ({ value: c.benchValue!, verdict: c.verdict, label: `${c.sha.slice(0, 7)} ${c.verdict}` }));
  return {
    id: a.id,
    kind: a.kind,
    branch: a.branch,
    status: a.status,
    hypothesis: latestAgentMessage(ledger, a.id),
    recent,
    stats: {
      candidates: mine.length,
      verified: mine.filter((c) => c.movedRatchet).length,
      rejected: mine.filter((c) => c.verdict === 'rejected' || c.verdict === 'failed_confirm').length,
      blocked: mine.filter((c) => c.verdict === 'blocked').length,
      spendUsd,
    },
    lastEventId: lastEventId(ledger, a.id),
    runnerInstanceId: a.runnerInstanceId,
  };
}

// ------------------------------------------------------------------ bench pipeline (derived from the newest candidate)

export function buildPipeline(cands: CandidateWithResults[]): BenchPipelineVM {
  const latest = cands.at(-1);
  const stage = (id: PipelineStage['id'], label: string, state: PipelineStage['state'], detail: string): PipelineStage => ({ id, label, state, detail });
  if (!latest) {
    return {
      candidate: null,
      queued: 0,
      stages: [
        stage('checkout', 'Check out sha', 'idle', 'waiting for a candidate'),
        stage('guardrails', 'Guardrails', 'idle', ''),
        stage('bench', 'Bench ×N', 'idle', ''),
        stage('confirm', 'Confirm on hidden trace', 'idle', ''),
        stage('verdict', 'Verdict', 'idle', ''),
      ],
    };
  }
  const bench = latest.results.filter((r) => r.phase === 'bench').at(-1);
  const confirm = latest.results.filter((r) => r.phase === 'confirm').at(-1);
  const pending = latest.verdict === 'pending';
  const guardFailed = bench?.guardrails.some((g) => !g.passed) ?? false;
  const stages: PipelineStage[] = [
    stage('checkout', 'Check out sha', 'done', latest.sha.slice(0, 7)),
    stage(
      'guardrails',
      'Guardrails',
      !bench ? (pending ? 'active' : 'idle') : guardFailed ? 'failed' : 'done',
      bench ? (guardFailed ? bench.guardrails.filter((g) => !g.passed).map((g) => g.name).join(', ') : `${bench.guardrails.length} passed`) : 'running…',
    ),
    stage('bench', `Bench ×${bench?.samples.length ?? 'N'}`, bench ? 'done' : guardFailed ? 'idle' : pending ? 'active' : 'idle', bench ? `median ${fmtNum(bench.stats.value)}` : 'queued'),
    stage(
      'confirm',
      'Confirm on hidden trace',
      confirm ? (latest.verdict === 'failed_confirm' ? 'failed' : 'done') : bench && pending && !guardFailed ? 'active' : 'idle',
      confirm ? `median ${fmtNum(confirm.stats.value)}` : pending ? 'running…' : 'skipped',
    ),
    stage('verdict', 'Verdict', pending ? 'active' : latest.verdict === 'verified' ? 'done' : 'failed', pending ? 'deciding…' : latest.verdict.replace('_', ' ')),
  ];
  return { candidate: { id: latest.id, sha: latest.sha.slice(0, 7), agent: latest.agentKind }, queued: Math.max(0, cands.filter((c) => c.verdict === 'pending').length - 1), stages };
}

// ------------------------------------------------------------------ detail

export function buildDetail(ledger: Ledger, id: string, meta: MetaView, now: number): OptimizationDetail {
  const summary = ledger.listOptimizations().find((o) => o.id === id);
  const opt = ledger.getOptimization(id);
  if (!summary || !opt) throw new Error(`optimization ${id} not found`);
  const baseline = ledger.getBaseline(id);
  const cands = ledger.candidatesWithResults(id);
  const spend = ledger.spendBreakdown(id);
  const agentSpend = new Map(spend.byAgent.map((a) => [a.agentId, a.total]));
  const agents = ledger.listAgents(id).map((a) => toAgentVM(ledger, a, cands, agentSpend.get(a.id) ?? 0));
  const lastMove = ledger.ratchetHistory(id).at(-1);
  const bestStats = lastMove ? cands.find((c) => c.id === lastMove.candidateId)?.results.filter((r) => r.phase === 'bench').at(-1)?.stats : undefined;
  const baseStats = baseline?.stats ?? {};
  const start = opt.startedAt;
  const elapsed = start ? Math.max(0, ((opt.endedAt ?? now) - start) / 1000) : 0;
  return {
    summary: toOptimization(summary, opt, meta, now),
    baseRef: opt.baseRef,
    minGain: opt.minGain,
    timeLimitS: opt.timeLimitS,
    startedAt: opt.startedAt,
    endedAt: opt.endedAt,
    elapsedS: elapsed,
    baselineSamples: baseline?.samples ?? [],
    baselineStats: baseline?.stats ?? {},
    noiseFloor: baseline?.noiseFloor ?? null,
    agents,
    pipeline: buildPipeline(cands),
    prUrl: meta.prUrl(id),
    receipt: null,
    calibration: null,
    configYaml: opt.configYaml,
    metrics: baseline ? metricOptions(opt.metric, opt.configYaml, baseStats) : [],
    compare: baseline ? compareRows(opt.metric, opt.direction, opt.configYaml, baseStats, bestStats) : [],
    spans: spanRows(baseStats, bestStats),
  };
}

// ------------------------------------------------------------------ winning change

const EXCERPT_LINES = 14;

/** First hunk of `patch` as typed lines (at most `EXCERPT_LINES`). */
export function hunkExcerpt(patch: string): DiffLine[] {
  const lines = patch.split('\n');
  const start = lines.findIndex((l) => l.startsWith('@@'));
  if (start < 0) return [];
  const out: DiffLine[] = [{ kind: 'hunk', text: lines[start]! }];
  for (const l of lines.slice(start + 1)) {
    if (l.startsWith('@@') || out.length >= EXCERPT_LINES) break;
    if (l.startsWith('\\')) continue; // "\ No newline at end of file"
    out.push({ kind: l.startsWith('+') ? 'add' : l.startsWith('-') ? 'del' : 'ctx', text: l.slice(1) });
  }
  return out;
}

/**
 * The squashed result as the pull request will show it. `files` is the diff of the best verified commit against
 * `baseRef` (GitHost.diff); `unavailable` explains a missing diff instead of inventing one.
 */
export function toWinningChange(ledger: Ledger, id: string, files: FileDiff[] | null, unavailable: string | null): WinningChangeVM {
  const opt = ledger.getOptimization(id);
  if (!opt) throw new Error(`optimization ${id} not found`);
  const moves = ledger.ratchetHistory(id);
  const best = moves.at(-1);
  const empty = { files: [], additions: 0, deletions: 0, excerpt: null };
  if (!best) return { commits: 0, sha: null, baseRef: opt.baseRef, ...empty, unavailable: 'No verified win yet.' };
  if (!files) return { commits: moves.length, sha: best.sha.slice(0, 7), baseRef: opt.baseRef, ...empty, unavailable: unavailable ?? 'Diff unavailable.' };
  const top = [...files].filter((f) => !f.binary && f.patch).sort((a, b) => b.additions + b.deletions - (a.additions + a.deletions))[0];
  return {
    commits: moves.length,
    sha: best.sha.slice(0, 7),
    baseRef: opt.baseRef,
    files: files.map((f) => ({ path: f.path, additions: f.additions, deletions: f.deletions, binary: f.binary })),
    additions: files.reduce((n, f) => n + f.additions, 0),
    deletions: files.reduce((n, f) => n + f.deletions, 0),
    excerpt: top ? { path: top.path, lines: hunkExcerpt(top.patch) } : null,
    unavailable: null,
  };
}

// ------------------------------------------------------------------ config

export function validateConfigText(text: string): ValidateConfigResult {
  const r = parseConfig(text);
  if (!r.ok) return { ok: false, errors: r.errors };
  const c = r.config;
  const v = c.objective.value;
  const summary: ConfigSummary = {
    name: c.objective.name ?? null,
    pack: c.pack ?? null,
    metric: c.objective.metric,
    direction: c.objective.direction,
    // Absent in ratchet.yaml means "calibrated from the baseline" (EngineApi.calibration shows the real figure); the default stands in here.
    minGain: c.objective.min_gain ?? CALIBRATION.fallbackGain,
    // Absent in ratchet.yaml means "calibrated from the baseline": null, not a made-up number.
    evaluator: { kind: c.evaluator.kind, ref: c.evaluator.ref, run: c.evaluator.run, repeats: c.evaluator.repeats ?? null, hardware: c.evaluator.hardware, holdout: c.evaluator.holdout ?? (c.evaluator.template ? (getTemplate(c.evaluator.template)?.holdout ?? null) : null), template: c.evaluator.template ?? null },
    environment: { image: c.environment?.image ?? null, machine: c.evaluator.hardware, region: c.environment?.region ?? null, network: c.environment?.network ?? [] },
    value: v ? { usdPerUnit: v.usd_per_unit, period: v.period, basis: v.basis, unit: v.unit ?? null } : null,
    guardrails: c.guardrails.map((g) => (g.kind === 'command' ? `run: ${g.run}` : `${g.metric} ${g.raw}`)),
    protectedGlobs: c.protected,
    protectedEntries: effectiveProtected(c).entries.map((e) => ({ glob: e.glob, reason: e.reason })),
    instanceTypes: c.agent.allowed_instance_types,
  };
  return { ok: true, summary };
}

// ------------------------------------------------------------------ report

export function buildReport(ledger: Ledger, id: string, meta: MetaView, now: number): string {
  const d = buildDetail(ledger, id, meta, now);
  const s = d.summary;
  const series = buildSeries(ledger, id, now);
  const cands = buildCandidates(ledger, id);
  const spend = buildSpend(ledger, id);
  const u = s.unit ? ` ${s.unit}` : '';
  const n = (v: number | null) => (v === null ? '—' : `${fmtNum(v)}${u}`);
  const lines: string[] = [
    `# Ratchet report: ${s.name}`,
    '',
    `Repository: ${s.repo} (${d.baseRef})  `,
    `Metric: ${s.metric} (${series.direction} is better)  `,
    `Status: ${s.status}`,
    '',
    '## Result',
    '',
    `**${n(s.baseline)} → ${n(s.best)}**${s.gainPct !== null ? ` (${s.gainPct.toFixed(1)}% better)` : ''} with ${s.verifiedWins} verified win${s.verifiedWins === 1 ? '' : 's'} out of ${s.candidateCount} candidates.`,
    '',
    '## Verified wins',
    '',
    ...(cands.filter((c) => c.movedRatchet).length
      ? cands.filter((c) => c.movedRatchet).map((c) => `- \`${c.sha}\` (${c.agent}) ${c.message}: ${n(c.benchValue)}, hidden-trace ${n(c.confirmValue)}`)
      : ['_None yet._']),
    '',
    '## Spend',
    '',
    `Total $${spend.total.toFixed(2)} of $${spend.budgetUsd.toFixed(2)} budget (${spend.estimated > 0 ? 'includes estimates' : 'reconciled'}).`,
    '',
    ...spend.byCategory.filter((c) => c.usd > 0).map((c) => `- ${c.label}: $${c.usd.toFixed(2)}`),
    '',
    '## All candidates',
    '',
    '| sha | agent | verdict | bench | vs baseline |',
    '|---|---|---|---|---|',
    ...cands.map((c) => `| ${c.sha} | ${c.agent} | ${c.verdict} | ${n(c.benchValue)} | ${c.vsBaselinePct === null ? '—' : `${c.vsBaselinePct.toFixed(1)}%`} |`),
    '',
  ];
  return lines.join('\n');
}

// ------------------------------------------------------------------ environments and templates

/** The receipt shown on Result and Environments (counts instead of ids; ids stay in the ledger). */
export function toReceiptVM(r: SandboxTeardownReceipt): TeardownReceiptVM {
  return {
    sandboxId: r.sandboxId,
    region: r.region,
    trigger: r.trigger,
    destroyedAt: r.destroyedAt,
    spendUsd: r.spendUsd,
    spendByCategory: Object.entries(r.spendByCategory)
      .filter(([, usd]) => usd > 0)
      .map(([k, usd]) => ({ label: CATEGORY_LABEL[k] ?? k, usd })),
    runs: r.runs.map((x) => ({ optimizationId: x.optimizationId, status: x.status, spendUsd: x.spendUsd })),
    removed: { instances: r.removed.instances.length, agentIdentities: r.removed.agentIdentities.length, secrets: r.removed.secrets.length, resources: r.removed.resources },
    nothingRunning: r.nothingRunning,
    orphans: r.orphans.map((o) => `${o.role} ${o.id} (${o.state})`),
    errors: r.errors,
    summary: r.summary,
  };
}

export function toTemplateVM(t: TemplateInfo): TemplateVM {
  return {
    id: t.id,
    kind: t.kind,
    category: t.category,
    title: t.title,
    description: t.description,
    measures: t.measures,
    holdout: t.holdout,
    machine: t.machine,
    evaluatorKind: t.evaluatorKind,
    objective: t.objective,
    files: t.files,
  };
}
