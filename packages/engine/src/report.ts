import type { Ledger } from '@ratchet/core';

const fmt = (n: number): string => String(Number(n.toPrecision(5)));

/** Fallback PR body when no `reportBuilder` is configured. A richer report builder replaces it. */
export function defaultReport(ledger: Ledger, optimizationId: string): string {
  const opt = ledger.getOptimization(optimizationId);
  if (!opt) throw new Error(`optimization ${optimizationId} not found`);
  const baseline = ledger.getBaseline(optimizationId);
  const best = ledger.currentBest(optimizationId);
  const history = ledger.ratchetHistory(optimizationId);
  const spend = ledger.spendBreakdown(optimizationId);
  const cands = ledger.candidatesWithResults(optimizationId);
  const counts = new Map<string, number>();
  for (const c of cands) counts.set(c.verdict, (counts.get(c.verdict) ?? 0) + 1);

  const lines: string[] = [];
  lines.push(`## Ratchet: ${opt.metric} (${opt.direction})`);
  lines.push('');
  if (baseline && best !== null) {
    const delta = opt.direction === 'minimize' ? baseline.value - best : best - baseline.value;
    const pct = baseline.value === 0 ? 0 : (delta / Math.abs(baseline.value)) * 100;
    lines.push(
      `**${fmt(baseline.value)} -> ${fmt(best)}** (${pct >= 0 ? '' : '-'}${fmt(Math.abs(pct))}% ${pct >= 0 ? 'better' : 'worse'}), ` +
        `noise floor ${fmt(baseline.noiseFloor)}, ${history.length} verified win${history.length === 1 ? '' : 's'}.`,
    );
    lines.push('');
  }
  lines.push('### Verified wins (ratchet order)');
  lines.push('');
  if (history.length === 0) lines.push('None.');
  else {
    lines.push('| # | Agent | Commit | From | To | Gain |');
    lines.push('|---|---|---|---|---|---|');
    history.forEach((m, i) => {
      const msg = cands.find((c) => c.id === m.candidateId)?.message.split('\n')[0] ?? '';
      lines.push(
        `| ${i + 1} | ${m.agentKind} | \`${m.sha.slice(0, 10)}\` ${msg ? `- ${msg.replace(/\|/g, '/')}` : ''} | ${fmt(m.fromValue)} | ${fmt(m.toValue)} | ${fmt(m.gainPct)}% |`,
      );
    });
  }
  lines.push('');
  lines.push('### All candidates');
  lines.push('');
  lines.push(
    [...counts.entries()]
      .sort()
      .map(([v, n]) => `${n} ${v}`)
      .join(', ') || 'none',
  );
  lines.push('');
  lines.push('### Spend');
  lines.push('');
  lines.push(`Total $${spend.total.toFixed(2)} of $${opt.budgetUsd.toFixed(2)} budget (estimated $${spend.estimated.toFixed(2)}).`);
  for (const [cat, usd] of Object.entries(spend.byCategory)) if (usd > 0) lines.push(`- ${cat}: $${usd.toFixed(2)}`);
  if (history.length > 0) lines.push(`- per verified win: $${(spend.total / history.length).toFixed(2)}`);
  lines.push('');
  lines.push('_Every commit above was re-benchmarked on trusted hardware and confirmed on a hidden trace before it moved the ratchet._');
  return lines.join('\n');
}
