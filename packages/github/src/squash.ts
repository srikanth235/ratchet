import type { AgentKind, Direction, Ledger } from '@ratchet/core';
import { gainPercent } from '@ratchet/core';

export interface SquashWin {
  sha: string;
  agentKind: AgentKind;
  /** Candidate commit message; the first line is used as the change summary. */
  message: string;
  fromValue: number;
  toValue: number;
}

export interface SquashPlanInput {
  metric: string;
  direction: Direction;
  /** Verified wins in ratchet order. */
  wins: SquashWin[];
}

export interface SquashPlan {
  title: string;
  /** Full commit message: title, body and a `Co-authored-by` trailer block. */
  message: string;
  /** Verified SHAs in ratchet order. */
  shas: string[];
  /**
   * SHA whose tree becomes the squash commit. Later wins are rebased onto earlier ones, so the final
   * verified SHA already contains every earlier win.
   */
  finalSha: string;
  coAuthors: string[];
}

const CO_AUTHORS: Record<AgentKind, string> = {
  'claude-code': 'Claude Code <noreply@anthropic.com>',
  codex: 'Codex <noreply@openai.com>',
  mock: 'Ratchet Mock Agent <mock-agent@ratchet.invalid>',
};

export function subjectOf(message: string): string {
  return message.split('\n')[0]?.trim() || '(no message)';
}

const fmt = (n: number) => (Number.isInteger(n) ? String(n) : String(Number(n.toPrecision(5))));

function pct(direction: Direction, from: number, to: number): string {
  const g = gainPercent(direction, from, to);
  return g === null ? 'n/a' : `${g >= 0 ? '' : '-'}${Math.abs(g).toFixed(1)}%`;
}

/** Build the squash commit message and the SHAs it covers. */
export function buildSquashPlan(input: SquashPlanInput): SquashPlan {
  const { wins, metric, direction } = input;
  if (wins.length === 0) throw new Error('cannot squash: no verified wins');
  const first = wins[0]!;
  const last = wins[wins.length - 1]!;
  const noun = wins.length === 1 ? 'verified win' : 'verified wins';
  const title = `ratchet: ${metric} ${fmt(first.fromValue)} → ${fmt(last.toValue)} (${pct(direction, first.fromValue, last.toValue)}, ${wins.length} ${noun})`;

  const body = wins.map(
    (w) => `- ${subjectOf(w.message)} (${w.agentKind}, ${w.sha.slice(0, 8)}, ${pct(direction, w.fromValue, w.toValue)})`,
  );
  const coAuthors = [...new Set(wins.map((w) => CO_AUTHORS[w.agentKind]))];
  const message = [title, '', ...body, '', ...coAuthors.map((c) => `Co-authored-by: ${c}`)].join('\n');
  return { title, message, shas: wins.map((w) => w.sha), finalSha: last.sha, coAuthors };
}

/** Squash plan for an optimization's verified wins, read from the ledger. */
export function buildSquashPlanFromLedger(ledger: Ledger, optimizationId: string): SquashPlan {
  const opt = ledger.getOptimization(optimizationId);
  if (!opt) throw new Error(`unknown optimization ${optimizationId}`);
  const messages = new Map(ledger.candidatesWithResults(optimizationId).map((c) => [c.id, c.message]));
  const wins = ledger.ratchetHistory(optimizationId).map((h) => ({
    sha: h.sha,
    agentKind: h.agentKind,
    message: messages.get(h.candidateId) ?? '',
    fromValue: h.fromValue,
    toValue: h.toValue,
  }));
  return buildSquashPlan({ metric: opt.metric, direction: opt.direction, wins });
}
