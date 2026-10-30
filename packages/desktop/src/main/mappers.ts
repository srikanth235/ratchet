import {
  parseConfig,
  spendEvents,
  type ActivityItem as LedgerActivity,
  type AgentRow,
  type CandidateWithResults,
  type Ledger,
  type OptimizationSummary,
  type SpendBreakdown,
} from '@ratchet/core';
import type {
  ActivityItem,
  AgentEventVM,
  AgentEventsPage,
  AgentVM,
  BenchPipelineVM,
  CandidateVM,
  ConfigSummary,
  Kpi,
  Optimization,
  OptimizationDetail,
  OptimizationStatus,
  PipelineStage,
  RatchetPoint,
  RatchetSeries,
  SpendVM,
  TeardownItem,
  ValidateConfigResult,
} from '../shared/types.js';

/** State the ledger does not hold (kept in the settings store by the service). */
export interface MetaView {
  prUrl(optimizationId: string): string | null;
  teardownDone(optimizationId: string): boolean;
}

const MIN = 60_000;

// ------------------------------------------------------------------ small pure helpers

/** Best-effort display unit from a metric key such as `http_req_duration.p95`. */
export function unitForMetric(metric: string): string {
  const m = metric.toLowerCase();
  if (/(^|[._])(rss|mem|memory|heap)/.test(m) || /_mb$/.test(m)) return 'MB';
  if (/(throughput|events_per_s|per_s|rps|qps)/.test(m)) return 'k/s';
  if (/(cold_start|\.s$|_s$|seconds)/.test(m)) return 's';
  if (/(duration|latency|render|\.p\d+|_ms$|ms)/.test(m)) return 'ms';
  return '';
}

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

export function median(xs: readonly number[]): number {
  if (xs.length === 0) return 0;
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m]! : (s[m - 1]! + s[m]!) / 2;
}

/** Evenly pick at most `n` values (always keeping the last). */
export function downsample(xs: readonly number[], n = 8): number[] {
  if (xs.length <= n) return [...xs];
  return Array.from({ length: n }, (_, i) => xs[Math.round((i * (xs.length - 1)) / (n - 1))]!);
}

const optName = (o: { name: string | null; repo: string; metric: string }): string => o.name ?? `${o.repo} ${o.metric}`;

// ------------------------------------------------------------------ optimizations

export function toOptimization(s: OptimizationSummary, meta: MetaView, now: number): Optimization {
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
  };
}

export function buildSeries(ledger: Ledger, id: string, now: number): RatchetSeries {
  const opt = ledger.getOptimization(id);
  if (!opt) throw new Error(`optimization ${id} not found`);
  const t0 = opt.startedAt ?? opt.createdAt;
  const minutes = (at: number) => Math.max(0, (at - t0) / MIN);
  const baseline = ledger.getBaseline(id);
  const moves = ledger.ratchetHistory(id);
  const cands = ledger.candidatesWithResults(id);

  const steps: { t: number; value: number }[] = [];
  if (baseline) steps.push({ t: 0, value: baseline.value });
  for (const m of moves) steps.push({ t: minutes(m.at), value: m.toValue });

  const points: RatchetPoint[] = cands
    .filter((c) => c.benchValue !== null)
    .map((c) => ({
      t: minutes(c.submittedAt),
      value: c.benchValue!,
      agent: c.agentKind,
      verified: c.movedRatchet,
      label: `${c.sha.slice(0, 7)} · ${c.message || c.verdict}`,
    }));

  const last = Math.max(0, ...steps.map((s) => s.t), ...points.map((p) => p.t));
  const live = opt.status === 'running' || opt.status === 'baselining';
  return {
    metric: opt.metric,
    unit: unitForMetric(opt.metric),
    direction: uiDirection(opt.direction),
    baseline: baseline?.value ?? null,
    steps,
    points,
    noiseFloor: baseline?.noiseFloor ?? null,
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

// ------------------------------------------------------------------ KPIs

export function buildKpis(ledger: Ledger, rows: OptimizationSummary[], now: number): Kpi[] {
  const moves = rows.flatMap((r) => ledger.ratchetHistory(r.id)).sort((a, b) => a.at - b.at);
  const wins = moves.length;
  const weekAgo = now - 7 * 24 * 60 * MIN;
  const winsThisWeek = moves.filter((m) => m.at >= weekAgo).length;

  const gains = rows.map((r) => r.gainPct).filter((g): g is number => g !== null && g > 0);
  const medianGain = median(gains);
  const gainTrend = downsample(moves.map((_, i) => median(moves.slice(0, i + 1).map((m) => m.gainPct))));

  const spend = rows.reduce((a, r) => a + r.spendUsd, 0);
  const ids = new Set(rows.map((r) => r.id));
  const events = ledger.db
    .select({ at: spendEvents.at, usd: spendEvents.usd, opt: spendEvents.optimizationId })
    .from(spendEvents)
    .all()
    .filter((e) => ids.has(e.opt))
    .sort((a, b) => a.at - b.at);
  let cum = 0;
  const spendTrend = downsample(events.map((e) => (cum += e.usd)));
  const dayStart = now - 24 * 60 * MIN;
  const today = events.filter((e) => e.at >= dayStart).reduce((a, e) => a + e.usd, 0);

  const cpw = wins > 0 ? spend / wins : 0;
  const winTrend = downsample(moves.map((_, i) => i + 1));

  return [
    { id: 'wins', label: 'Verified wins', value: String(wins), delta: `+${winsThisWeek} this week`, trend: winTrend },
    { id: 'gain', label: 'Median gain', value: gains.length ? medianGain.toFixed(0) : '—', unit: gains.length ? '%' : '', delta: `${gains.length} optimization${gains.length === 1 ? '' : 's'}`, trend: gainTrend },
    { id: 'spend', label: 'Spend', value: `$${spend.toFixed(0)}`, delta: `$${today.toFixed(0)} last 24h`, trend: spendTrend },
    { id: 'cpw', label: 'Cost per win', value: wins ? `$${cpw.toFixed(1)}` : '—', delta: wins ? `${wins} verified win${wins === 1 ? '' : 's'}` : 'no wins yet', trend: downsample(moves.map((_, i) => (i + 1 === 0 ? 0 : spend / (i + 1)))) },
  ];
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

export function buildTeardown(status: string, agentCount: number, tornDown: boolean): TeardownItem[] {
  const stopped = ['completed', 'stopped', 'failed'].includes(status) && agentCount >= 0;
  return [
    { id: 'agents', label: 'Agents stopped', done: stopped },
    { id: 'identities', label: 'Agent IAM identities revoked and deleted', done: stopped },
    { id: 'instances', label: 'Runner, experiment and bench instances terminated', done: stopped },
    { id: 'secrets', label: 'Per-agent secrets deleted from the sandbox', done: tornDown },
    { id: 'sandbox', label: 'Sandbox resources destroyed (tofu destroy)', done: tornDown },
  ];
}

export function buildDetail(ledger: Ledger, id: string, meta: MetaView, now: number): OptimizationDetail {
  const summary = ledger.listOptimizations().find((o) => o.id === id);
  const opt = ledger.getOptimization(id);
  if (!summary || !opt) throw new Error(`optimization ${id} not found`);
  const baseline = ledger.getBaseline(id);
  const cands = ledger.candidatesWithResults(id);
  const spend = ledger.spendBreakdown(id);
  const agentSpend = new Map(spend.byAgent.map((a) => [a.agentId, a.total]));
  const agents = ledger.listAgents(id).map((a) => toAgentVM(ledger, a, cands, agentSpend.get(a.id) ?? 0));
  const start = opt.startedAt;
  const elapsed = start ? Math.max(0, ((opt.endedAt ?? now) - start) / 1000) : 0;
  return {
    summary: toOptimization(summary, meta, now),
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
    teardown: buildTeardown(opt.status, agents.length, meta.teardownDone(id)),
    configYaml: opt.configYaml,
  };
}

// ------------------------------------------------------------------ config

export function validateConfigText(text: string): ValidateConfigResult {
  const r = parseConfig(text);
  if (!r.ok) return { ok: false, errors: r.errors };
  const c = r.config;
  const summary: ConfigSummary = {
    metric: c.objective.metric,
    direction: c.objective.direction,
    minGain: c.objective.min_gain,
    benchRun: c.bench.run,
    repeats: c.bench.repeats,
    hardware: c.bench.hardware,
    guardrails: c.guardrails.map((g) => (g.kind === 'command' ? `run: ${g.run}` : `${g.metric} ${g.raw}`)),
    protectedGlobs: c.protected,
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
