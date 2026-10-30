import { describe, expect, it } from 'vitest';
import { buildContext, buildInitialPrompt, buildResumeMessage, formatDuration, improvementPct, type HistoryEntry } from '../src/index.js';
import { baseConfig, type Sandbox } from './harness.js';

const sb = { remote: '/nowhere.git' } as Sandbox;
const config = baseConfig(sb, { deadlineAt: 10_000_000 + 3 * 3_600_000 + 25 * 60_000, budgetUsd: 20 });
const entry = (over: Partial<HistoryEntry> = {}): HistoryEntry => ({ n: 1, sha: 'abcdef1234567890', verdict: 'rejected', reasons: ['gain 0.8% < noise floor 4%', 'a|b'], best: 20, at: 0, ...over });

describe('formatDuration', () => {
  it('formats hours and minutes, never negative', () => {
    expect(formatDuration(3 * 3_600_000 + 25 * 60_000)).toBe('3h 25m');
    expect(formatDuration(59 * 60_000)).toBe('59m');
    expect(formatDuration(-5)).toBe('0m');
  });
});

describe('improvementPct', () => {
  it('is positive when the metric moved in the objective direction', () => {
    expect(improvementPct(config, 20, 5)).toBe('75.0%');
    expect(improvementPct(config, 20, 25)).toBe('-25.0%');
    expect(improvementPct({ ...config, config: { ...config.config, objective: { ...config.config.objective, direction: 'maximize' } } }, 100, 150)).toBe('50.0%');
  });
});

describe('buildContext', () => {
  const text = buildContext({ config, bestValue: 12.5, history: [entry(), entry({ n: 2, verdict: 'verified', reasons: [], best: 12.5, sha: '1111111122222222' })], now: 10_000_000, spendUsd: 4.5, turn: 3, bestSha: '11111111' });
  it('states the objective, baseline and current best', () => {
    expect(text).toContain('`http_req_duration.p95` (minimize)');
    expect(text).toContain('more than 3.0%');
    expect(text).toContain('Baseline: 20');
    expect(text).toContain('Current ratchet best: 12.5 (commit 11111111); improvement so far 37.5%');
  });
  it('shows remaining time and budget', () => {
    expect(text).toContain('Time remaining: 3h 25m');
    expect(text).toContain('Budget remaining: $15.50 of $20.00');
  });
  it('lists the verdict history as a table, escaping pipes', () => {
    expect(text).toContain('| 1 | abcdef12 | rejected | 20 | gain 0.8% < noise floor 4%; a/b |');
    expect(text).toContain('| 2 | 11111111 | verified | 12.5 | - |');
  });
  it('lists rules: protected paths and guardrails', () => {
    expect(text).toContain('`bench/**`, `ratchet.yaml`');
    expect(text).toContain('`pnpm test` must pass; `error_rate` < 0.001');
  });
  it('says so when there is no history yet', () => {
    expect(buildContext({ config, bestValue: 20, history: [], now: 0, spendUsd: 0, turn: 1 })).toContain('None yet.');
  });
});

describe('prompts', () => {
  it('initial prompt = brief + context', () => {
    const p = buildInitialPrompt(config, 'CTX');
    expect(p.startsWith('# Brief')).toBe(true);
    expect(p).toContain('.ratchet/context.md');
    expect(p.trimEnd().endsWith('CTX')).toBe(true);
  });
  it('resume message for a verified / rejected / no-change turn', () => {
    const base = { bestValue: 12, config, now: 10_000_000, spendUsd: 1, notes: ['NOTE'], userMessages: ['hi'] };
    const verified = buildResumeMessage({ ...base, entry: entry({ verdict: 'verified', reasons: ['p95 -31%'] }) });
    expect(verified).toContain('VERIFIED');
    expect(verified).toContain('- p95 -31%');
    expect(verified).toContain('The ratchet moved to your commit.');
    expect(verified).toContain('NOTE');
    expect(verified).toContain('Message from the user: hi');
    expect(buildResumeMessage({ ...base, entry: entry() })).toContain('refs/ratchet/accepted');
    expect(buildResumeMessage({ ...base, noChange: true })).toContain('No change to the repository');
  });
});
