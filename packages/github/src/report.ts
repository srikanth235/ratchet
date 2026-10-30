import {
  gainPercent,
  parseConfig,
  type AgentKind,
  type BenchStats,
  type CandidateVerdict,
  type Direction,
  type Ledger,
  type SpendCategory,
} from '@ratchet/core';
import { subjectOf } from './squash.js';

export interface ReportWin {
  index: number;
  candidateId: string;
  agentId: string;
  agentKind: AgentKind;
  branch: string;
  sha: string;
  change: string;
  benchValue: number | null;
  confirmValue: number | null;
  fromValue: number;
  toValue: number;
  gainPct: number;
  /** Agent spend since that agent's previous verified win (or its start), up to this win. */
  costUsd: number;
}

export interface ReportAgent {
  agentId: string;
  kind: AgentKind;
  branch: string;
  candidates: number;
  verified: number;
  spendUsd: number;
}

export interface ResultReportData {
  optimizationId: string;
  name: string | null;
  repo: string;
  baseRef: string;
  metric: string;
  direction: Direction;
  status: string;
  baseline: { value: number; noiseFloor: number; stats: BenchStats } | null;
  best: { value: number; stats: BenchStats | null; sha: string | null };
  gainPct: number | null;
  wins: ReportWin[];
  agents: ReportAgent[];
  verdicts: Record<CandidateVerdict, number>;
  spend: {
    total: number;
    estimated: number;
    actual: number;
    byCategory: Record<SpendCategory, number>;
  };
  costPerVerifiedWinUsd: number | null;
  reproduction: { commands: string[] };
}

export interface ResultReport {
  data: ResultReportData;
  markdown: string;
  /** `data` pretty-printed. */
  json: string;
}

const PERCENTILE_ORDER = ['value', 'min', 'mean', 'median', 'p50', 'p75', 'p90', 'p95', 'p99', 'max', 'stddev'];

export function buildResultReport(ledger: Ledger, optimizationId: string): ResultReport {
  const opt = ledger.getOptimization(optimizationId);
  if (!opt) throw new Error(`unknown optimization ${optimizationId}`);
  const summary = ledger.listOptimizations().find((o) => o.id === optimizationId);
  const baseline = ledger.getBaseline(optimizationId);
  const candidates = ledger.candidatesWithResults(optimizationId);
  const byId = new Map(candidates.map((c) => [c.id, c]));
  const history = ledger.ratchetHistory(optimizationId);
  const spend = ledger.spendBreakdown(optimizationId);

  const agentCost = (agentId: string, after: number, upTo: number): number => {
    const row = ledger.sqlite
      .prepare('select coalesce(sum(usd), 0) as t from spend_events where optimization_id = ? and agent_id = ? and at > ? and at <= ?')
      .get(optimizationId, agentId, after, upTo) as { t: number };
    return row.t;
  };
  const lastWinAt = new Map<string, number>();
  const wins: ReportWin[] = history.map((h, i) => {
    const c = byId.get(h.candidateId)!;
    const costUsd = agentCost(h.agentId, lastWinAt.get(h.agentId) ?? Number.NEGATIVE_INFINITY, h.at);
    lastWinAt.set(h.agentId, h.at);
    return {
      index: i + 1,
      candidateId: h.candidateId,
      agentId: h.agentId,
      agentKind: h.agentKind,
      branch: c.agentBranch,
      sha: h.sha,
      change: subjectOf(c.message),
      benchValue: c.benchValue,
      confirmValue: c.confirmValue,
      fromValue: h.fromValue,
      toValue: h.toValue,
      gainPct: h.gainPct,
      costUsd,
    };
  });

  const verdicts: Record<CandidateVerdict, number> = { pending: 0, verified: 0, rejected: 0, failed_confirm: 0, blocked: 0 };
  for (const c of candidates) verdicts[c.verdict]++;

  const bestCandidate = history.length ? byId.get(history[history.length - 1]!.candidateId) : undefined;
  const bestStats = bestCandidate?.results.filter((r) => r.phase === 'bench').at(-1)?.stats ?? null;

  const agents: ReportAgent[] = ledger.listAgents(optimizationId).map((a) => ({
    agentId: a.id,
    kind: a.kind,
    branch: a.branch,
    candidates: candidates.filter((c) => c.agentId === a.id).length,
    verified: wins.filter((w) => w.agentId === a.id).length,
    spendUsd: spend.byAgent.find((s) => s.agentId === a.id)?.total ?? 0,
  }));

  const bestValue = summary?.bestValue ?? baseline?.value ?? 0;
  const benchCmd = (() => {
    const parsed = parseConfig(opt.configYaml);
    return parsed.ok ? parsed.config.bench.run : null;
  })();
  const finalSha = wins.at(-1)?.sha;
  const reproduction = [
    `git fetch origin ${opt.baseRef} ${agents.map((a) => a.branch).join(' ')}`.trim(),
    ...(finalSha
      ? [`git checkout ${finalSha}`]
      : [`git checkout ${opt.baseRef}`]),
    benchCmd ? benchCmd : `# run the benchmark configured in ratchet.yaml (bench.run)`,
  ];

  const data: ResultReportData = {
    optimizationId,
    name: opt.name,
    repo: opt.repo,
    baseRef: opt.baseRef,
    metric: opt.metric,
    direction: opt.direction,
    status: opt.status,
    baseline: baseline ? { value: baseline.value, noiseFloor: baseline.noiseFloor, stats: baseline.stats } : null,
    best: { value: bestValue, stats: bestStats, sha: finalSha ?? null },
    gainPct: summary?.gainPct ?? null,
    wins,
    agents,
    verdicts,
    spend: { total: spend.total, estimated: spend.estimated, actual: spend.actual, byCategory: spend.byCategory },
    costPerVerifiedWinUsd: wins.length ? spend.total / wins.length : null,
    reproduction: { commands: reproduction },
  };
  return { data, markdown: renderReportMarkdown(data), json: JSON.stringify(data, null, 2) };
}

// ---- markdown ---------------------------------------------------------------

const num = (n: number) => (Number.isInteger(n) ? String(n) : String(Number(n.toPrecision(5))));
const usd = (n: number) => `$${n.toFixed(2)}`;
/** Relative change of the metric (not the gain): a 56% latency gain renders as −56.3%, a 10% throughput gain as +10.0%. */
const signedPct = (gain: number | null, direction: Direction) => {
  if (gain === null) return 'n/a';
  const change = direction === 'minimize' ? -gain : gain;
  return `${change >= 0 ? '+' : '−'}${Math.abs(change).toFixed(1)}%`;
};
const cell = (s: string) => s.replace(/\|/g, '\\|').replace(/\n/g, ' ');
const table = (head: string[], rows: string[][]) =>
  [`| ${head.join(' | ')} |`, `| ${head.map(() => '---').join(' | ')} |`, ...rows.map((r) => `| ${r.map(cell).join(' | ')} |`)].join('\n');

/** Pretty "better" sign for a raw stat change: relative change in the metric's units, direction-aware. */
function statChange(direction: Direction, from: number, to: number): string {
  const g = gainPercent(direction, from, to);
  return signedPct(g, direction);
}

export function renderReportMarkdown(d: ResultReportData): string {
  const out: string[] = [];
  out.push(`# Ratchet result: ${d.name ?? d.optimizationId}`, '');
  if (d.baseline) {
    out.push(
      `**${d.metric}: ${num(d.baseline.value)} → ${num(d.best.value)} (${signedPct(d.gainPct, d.direction)})** — ` +
        `${d.wins.length} verified ${d.wins.length === 1 ? 'win' : 'wins'}, ${usd(d.spend.total)} spent` +
        (d.costPerVerifiedWinUsd !== null ? `, ${usd(d.costPerVerifiedWinUsd)} per verified win` : '') +
        '.',
    );
  } else {
    out.push('_No baseline recorded yet._');
  }
  out.push('', `Repo \`${d.repo}\` · base \`${d.baseRef}\` · ${d.direction} · status ${d.status}`, '');

  if (d.baseline) {
    out.push('## Percentiles', '');
    const keys = new Set([...Object.keys(d.baseline.stats), ...Object.keys(d.best.stats ?? {})]);
    const ordered = [...keys].sort((a, b) => {
      const ia = PERCENTILE_ORDER.indexOf(a);
      const ib = PERCENTILE_ORDER.indexOf(b);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
    });
    out.push(
      table(
        ['Stat', 'Baseline', 'Best', 'Change'],
        ordered.map((k) => {
          const from = d.baseline!.stats[k];
          const to = d.best.stats?.[k] ?? (k === 'value' ? d.best.value : undefined);
          return [
            k === 'value' ? `value (${d.metric})` : k,
            from === undefined ? '–' : num(from),
            to === undefined ? '–' : num(to),
            from !== undefined && to !== undefined ? statChange(d.direction, from, to) : '–',
          ];
        }),
      ),
      '',
      `Noise floor: ${num(d.baseline.noiseFloor)}`,
      '',
    );
  }

  out.push('## Verified wins', '');
  if (d.wins.length === 0) out.push('_None._', '');
  else
    out.push(
      table(
        ['#', 'Agent', 'SHA', 'Change', 'Bench', 'Confirm', 'Step', 'Cost'],
        d.wins.map((w) => [
          String(w.index),
          w.agentKind,
          `\`${w.sha.slice(0, 8)}\``,
          w.change,
          w.benchValue === null ? '–' : num(w.benchValue),
          w.confirmValue === null ? '–' : num(w.confirmValue),
          signedPct(w.gainPct, d.direction),
          usd(w.costUsd),
        ]),
      ),
      '',
      '_Cost: agent spend since that agent’s previous verified win._',
      '',
    );

  const v = d.verdicts;
  out.push(
    '## Candidates',
    '',
    table(
      ['Verified', 'Rejected', 'Failed confirm', 'Blocked', 'Pending'],
      [[v.verified, v.rejected, v.failed_confirm, v.blocked, v.pending].map(String)],
    ),
    '',
  );

  if (d.agents.length) {
    out.push(
      '## Agents',
      '',
      table(
        ['Agent', 'Branch', 'Candidates', 'Verified', 'Spend'],
        d.agents.map((a) => [a.kind, `\`${a.branch}\``, String(a.candidates), String(a.verified), usd(a.spendUsd)]),
      ),
      '',
    );
  }

  out.push(
    '## Spend',
    '',
    table(
      ['Category', 'USD'],
      [
        ...Object.entries(d.spend.byCategory)
          .filter(([, n]) => n !== 0)
          .map(([k, n]) => [k, usd(n)]),
        ['**total**', `**${usd(d.spend.total)}**`],
      ],
    ),
    '',
    `${usd(d.spend.actual)} reconciled, ${usd(d.spend.estimated)} estimated. Cost per verified win: ${
      d.costPerVerifiedWinUsd === null ? 'n/a' : usd(d.costPerVerifiedWinUsd)
    }.`,
    '',
    '## Reproduce',
    '',
    '```sh',
    ...d.reproduction.commands,
    '```',
    '',
  );
  return out.join('\n');
}
