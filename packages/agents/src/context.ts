import type { CandidateVerdict, RunnerConfig } from '@ratchet/core';

export interface HistoryEntry {
  n: number;
  sha: string;
  verdict: CandidateVerdict;
  reasons: string[];
  /** Ratchet best reported with this verdict. */
  best: number;
  at: number;
}

export interface ContextInput {
  config: RunnerConfig;
  bestValue: number;
  history: readonly HistoryEntry[];
  now: number;
  spendUsd: number;
  turn: number;
  /** Short sha of the commit the agent's tree is based on (the ratchet best, once known). */
  bestSha?: string | undefined;
}

export function formatDuration(ms: number): string {
  if (ms <= 0) return '0m';
  const totalMin = Math.floor(ms / 60_000);
  const h = Math.floor(totalMin / 60);
  const m = totalMin % 60;
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

const fmt = (n: number): string => (Number.isInteger(n) ? String(n) : String(Math.round(n * 1000) / 1000));
const usd = (n: number): string => `$${n.toFixed(2)}`;

export function improvementPct(config: RunnerConfig, from: number, to: number): string {
  if (from === 0) return 'n/a';
  const gain = config.config.objective.direction === 'minimize' ? (from - to) / from : (to - from) / from;
  return `${gain >= 0 ? '' : '-'}${Math.abs(gain * 100).toFixed(1)}%`;
}

/** The generated `.ratchet/context.md`: everything the agent needs to know that RATCHET.md cannot say statically. */
export function buildContext(input: ContextInput): string {
  const { config, history } = input;
  const o = config.config.objective;
  const remainingMs = config.deadlineAt - input.now;
  const remainingUsd = Math.max(0, config.budgetUsd - input.spendUsd);
  const lines: string[] = [];
  lines.push('# Ratchet context (generated; refreshed before every turn)', '');
  lines.push('## Objective');
  lines.push(`- Metric: \`${o.metric}\` (${o.direction}). A candidate must beat the current best by more than ${(o.min_gain * 100).toFixed(1)}% and more than the benchmark noise floor.`);
  lines.push(`- Baseline: ${fmt(config.baselineValue)}`);
  lines.push(`- Current ratchet best: ${fmt(input.bestValue)}${input.bestSha ? ` (commit ${input.bestSha})` : ''}; improvement so far ${improvementPct(config, config.baselineValue, input.bestValue)}`);
  lines.push('');
  lines.push('## Limits');
  lines.push(`- Time remaining: ${formatDuration(remainingMs)}`);
  lines.push(`- Budget remaining: ${usd(remainingUsd)} of ${usd(config.budgetUsd)}`);
  lines.push(`- Turn: ${input.turn}; candidates submitted: ${history.length}`);
  lines.push('');
  lines.push('## History of your candidates');
  if (history.length === 0) lines.push('None yet.');
  else {
    lines.push('| # | commit | verdict | best after | notes |', '|---|---|---|---|---|');
    for (const h of history) lines.push(`| ${h.n} | ${h.sha.slice(0, 8)} | ${h.verdict} | ${fmt(h.best)} | ${h.reasons.join('; ').replace(/\|/g, '/') || '-'} |`);
  }
  lines.push('');
  lines.push('## Rules');
  lines.push(`- Protected paths (a candidate touching them is blocked): ${config.config.protected.length ? config.config.protected.map((p) => `\`${p}\``).join(', ') : 'none'}`);
  const guardrails = config.config.guardrails.map((g) => (g.kind === 'command' ? `\`${g.run}\` must pass` : `\`${g.metric}\` ${g.raw}`));
  lines.push(`- Guardrails checked on every candidate: ${guardrails.length ? guardrails.join('; ') : 'none'}`);
  lines.push('- The trusted benchmark runs on pinned hardware and a hidden trace; your own measurements only guide you.');
  lines.push('');
  lines.push('## How to submit');
  lines.push('- Commit one idea per commit. The runner submits your newest commit (or commits a dirty tree for you) with the trailer `Ratchet-Candidate: true`; do not push yourself.');
  lines.push('- After each verdict you are resumed with the result. When another agent moves the ratchet best, the runner rebases your branch onto it (conflicts resolved in favour of the best).');
  lines.push('- To abandon rejected work and start from the last accepted tree: `git reset --hard refs/ratchet/accepted`.');
  return `${lines.join('\n')}\n`;
}

export function buildInitialPrompt(config: RunnerConfig, context: string): string {
  return [
    config.brief.trimEnd(),
    '',
    '---',
    '',
    'You are running under Ratchet. The generated context below is also written to `.ratchet/context.md` and refreshed before every turn; re-read it when you resume.',
    'Work in this repository, make one improvement, verify it with the tests and your own measurements, commit it, and end your turn. Ratchet will submit the commit to the trusted benchmark and resume you with the verdict.',
    '',
    context.trimEnd(),
    '',
  ].join('\n');
}

export interface ResumeInput {
  entry?: HistoryEntry | undefined;
  bestValue: number;
  config: RunnerConfig;
  now: number;
  spendUsd: number;
  notes: string[];
  userMessages: string[];
  /** Why there is no verdict (agent made no change). */
  noChange?: boolean;
}

export function buildResumeMessage(input: ResumeInput): string {
  const { config } = input;
  const parts: string[] = [];
  if (input.entry) {
    const e = input.entry;
    parts.push(`Verdict for candidate ${e.n} (${e.sha.slice(0, 8)}): ${e.verdict.toUpperCase()}.`);
    if (e.reasons.length) parts.push(e.reasons.map((r) => `- ${r}`).join('\n'));
    if (e.verdict === 'verified') parts.push('The ratchet moved to your commit.');
    else parts.push('The ratchet did not move. Your commit is still in your tree; use `git reset --hard refs/ratchet/accepted` to drop it.');
  } else if (input.noChange) {
    parts.push('No change to the repository was detected after your last turn, so nothing was submitted. Try a different idea, or finish if you are out of ideas.');
  }
  parts.push(`Current ratchet best: ${fmt(input.bestValue)} ${config.config.objective.metric} (baseline ${fmt(config.baselineValue)}).`);
  parts.push(`Time remaining: ${formatDuration(config.deadlineAt - input.now)}. Budget remaining: ${usd(Math.max(0, config.budgetUsd - input.spendUsd))}.`);
  for (const n of input.notes) parts.push(n);
  for (const u of input.userMessages) parts.push(`Message from the user: ${u}`);
  parts.push('`.ratchet/context.md` has been refreshed. Continue with your next idea.');
  return parts.join('\n\n');
}
