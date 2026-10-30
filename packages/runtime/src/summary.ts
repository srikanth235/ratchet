import { mkdirSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { VERDICTS, type CandidateVerdict, type OptimizationSummary, type SpendBreakdown, type ValueEstimate } from '@ratchet/core';

/** What `simulate --summary <file>` writes, and what the CLI prints at the end of a run. */
export interface SimulationSummary {
  project: string;
  pack: string | null;
  optimizationId: string;
  objective: string;
  metric: string;
  status: string;
  baseline: number | null;
  best: number | null;
  /** Improvement vs baseline in percent (positive = better in the metric's direction). */
  gainPct: number | null;
  /** Verified ratchet moves. */
  wins: number;
  candidates: number;
  /** Candidates per final verdict; every verdict is present, 0 when none. */
  verdicts: Record<CandidateVerdict, number>;
  spendUsd: number;
  spend: { estimatedUsd: number; actualUsd: number; byCategory: Record<string, number> };
  /** Dollar value of the win (`objective.value` in ratchet.yaml); null when the config has no value model. */
  value: { improvement: number; unit?: string; usdPerYear: number; basis: string; returnOnSpend: number | null } | null;
  /** The pull request as the git host returned it (a `file://` URL in simulation). */
  pr: string | null;
  /** The same as a filesystem path when the host is local, else the URL. */
  prPath: string | null;
  workRoot: string;
  /** The ledger SQLite file of the run. */
  db: string;
}

export interface SummaryInput {
  project: string;
  pack: string | null | undefined;
  /** `objective.name ?? objective.metric`. */
  objective: string;
  summary: OptimizationSummary;
  candidates: ReadonlyArray<{ verdict: CandidateVerdict }>;
  spend: SpendBreakdown;
  value: ValueEstimate | null;
  roi: number | null;
  pr?: string | undefined;
  workRoot: string;
  db: string;
}

/** `file:///x/y` becomes `/x/y`; anything else is returned as is. */
export function prToPath(pr: string): string {
  if (!pr.startsWith('file:')) return pr;
  try {
    return fileURLToPath(pr);
  } catch {
    return pr;
  }
}

export function buildSimulationSummary(i: SummaryInput): SimulationSummary {
  const verdicts = Object.fromEntries(VERDICTS.map((v) => [v, 0])) as Record<CandidateVerdict, number>;
  for (const c of i.candidates) verdicts[c.verdict] += 1;
  return {
    project: i.project,
    pack: i.pack ?? null,
    optimizationId: i.summary.id,
    objective: i.objective,
    metric: i.summary.metric,
    status: i.summary.status,
    baseline: i.summary.baselineValue,
    best: i.summary.bestValue,
    gainPct: i.summary.gainPct,
    wins: i.summary.verifiedWins,
    candidates: i.summary.candidateCount,
    verdicts,
    spendUsd: i.summary.spendUsd,
    spend: {
      estimatedUsd: i.spend.estimated,
      actualUsd: i.spend.actual,
      byCategory: { ...i.spend.byCategory },
    },
    value: i.value
      ? { improvement: i.value.improvement, ...(i.value.unit ? { unit: i.value.unit } : {}), usdPerYear: i.value.usdPerYear, basis: i.value.basis, returnOnSpend: i.roi }
      : null,
    pr: i.pr ?? null,
    prPath: i.pr ? prToPath(i.pr) : null,
    workRoot: i.workRoot,
    db: i.db,
  };
}

/** Write the summary as pretty JSON (creating parent directories; atomic rename so readers never see half a file). Returns the absolute path. */
export function writeSimulationSummary(path: string, summary: SimulationSummary): string {
  const file = resolve(path);
  mkdirSync(dirname(file), { recursive: true });
  const tmp = `${file}.${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(summary, null, 2) + '\n');
  renameSync(tmp, file);
  return file;
}
