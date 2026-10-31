import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ApproveEnvironmentOptions, EnvironmentApproval, EnvironmentDraft, EnvironmentDraftRecord, GitHost } from '@ratchet/core';
import { DEFAULT_ENVIRONMENT_INFRA_DIR, draftPassed } from '@ratchet/core';
import { EngineError } from '../errors.js';

const usd = (n: number): string => `$${n.toFixed(2)}`;

/** The pull request description of an approved environment: what it is, what it costs, what Ratchet checked, what changes. */
export function buildEnvironmentPullRequest(record: Pick<EnvironmentDraftRecord, 'goal' | 'id'>, draft: EnvironmentDraft): { title: string; body: string; commitMessage: string } {
  const lines: string[] = [];
  lines.push(`Ratchet environment for: ${record.goal.trim()}`, '');
  lines.push(`**Scope: ${draft.scope}.** ${draft.reason}`, '');
  if (draft.resources.length > 0) {
    lines.push('| What gets created | Machine | $/h |', '|---|---|---|');
    for (const r of draft.resources) lines.push(`| ${r.name}: ${r.detail} | ${r.machine} | ${usd(r.usdPerHour)} |`);
    lines.push('');
  }
  lines.push(`**Cost:** ${usd(draft.cost.usdPerHour)}/h, ready in ~${draft.cost.startupMinutes} min, about ${usd(draft.cost.estimateForRunUsd)} for the run's time limit.`, '');
  lines.push("**Ratchet's checks**", ...draft.checks.map((c) => `- ${c.ok ? 'passed' : 'FAILED'}: ${c.label}. ${c.detail}`), '');
  lines.push('**Files**', ...draft.files.map((f) => `- \`${f.path}\``), '');
  lines.push(
    `Once this is on the protected ref the environment is locked: \`${DEFAULT_ENVIRONMENT_INFRA_DIR}/**\` and \`ratchet.yaml\` join Ratchet's protected paths and any candidate that touches them is blocked. Agents change code only.`,
    '',
    `Drafted by ${draft.agent} in ${Math.round(draft.durationS)} s (draft ${record.id}).`,
  );
  const title = `Ratchet: ${draft.scope === 'none' ? 'no extra infrastructure' : draft.scope === 'single' ? 'one pinned machine' : 'full-stack environment'} for "${record.goal.trim().slice(0, 60)}"`;
  return { title, body: lines.join('\n'), commitMessage: `Ratchet: ${draft.scope} environment\n\n${draft.reason}` };
}

export interface CommitDraftOptions extends ApproveEnvironmentOptions {
  /** Called with the repo's current ratchet.yaml: refuse when it is not the one the draft was made from. */
  expectedConfigYaml?: string;
  configPath: string;
  scratchDir?: string;
  now(): number;
  /** Suffix for the PR branch (default: the draft id). */
  branchSuffix: string;
}

/**
 * Writes an approved draft to the protected ref: straight onto `baseRef` (`commit`) or on a branch with a pull request the user
 * merges (`pr`, the default). Refuses a draft whose checks did not all pass, and (when `expectedConfigYaml` is given) a base whose
 * ratchet.yaml changed since the draft was made, because the draft carries a whole ratchet.yaml.
 */
export async function commitDraft(git: GitHost, record: EnvironmentDraftRecord, opts: CommitDraftOptions): Promise<EnvironmentApproval> {
  const draft = record.draft;
  if (!draft) throw new EngineError('invalid_state', `draft ${record.id} has nothing to approve`);
  if (!draftPassed(draft)) {
    const failed = draft.checks.filter((c) => !c.ok).map((c) => c.label);
    throw new EngineError('invalid_state', `draft ${record.id} cannot be approved: ${failed.length > 0 ? `these checks did not pass: ${failed.join(', ')}` : 'it has no checks'}`);
  }
  if (!git.commitFiles) throw new EngineError('unavailable', 'this git host cannot commit files, so the environment cannot be written to the repository');
  const mode = opts.mode ?? 'pr';

  if (opts.expectedConfigYaml !== undefined) {
    const scratch = opts.scratchDir ?? tmpdir();
    await mkdir(scratch, { recursive: true });
    const dir = await mkdtemp(join(scratch, 'ratchet-approve-'));
    try {
      await git.checkout(record.baseRef, dir);
      const now = await readFile(join(dir, opts.configPath), 'utf8').catch(() => undefined);
      if (now !== opts.expectedConfigYaml) {
        throw new EngineError('invalid_state', `${opts.configPath} changed on ${record.baseRef} since this environment was drafted; draft it again so it is based on the current file`);
      }
    } finally {
      await rm(dir, { recursive: true, force: true }).catch(() => undefined);
    }
  }

  const pr = buildEnvironmentPullRequest(record, draft);
  const files = draft.files.map((f) => ({ path: f.path, content: f.content }));
  const paths = files.map((f) => f.path);
  if (mode === 'commit') {
    const { sha } = await git.commitFiles({ baseRef: record.baseRef, branch: record.baseRef, message: pr.commitMessage, files, update: true });
    return { mode, sha, files: paths, locked: true, at: opts.now() };
  }
  let branch = `ratchet/environment-${opts.branchSuffix}`;
  let sha: string;
  try {
    ({ sha } = await git.commitFiles({ baseRef: record.baseRef, branch, message: pr.commitMessage, files }));
  } catch (err) {
    // most likely the branch exists from an earlier attempt: take a fresh name once, then give up
    branch = `${branch}-${opts.now().toString(36)}`;
    try {
      ({ sha } = await git.commitFiles({ baseRef: record.baseRef, branch, message: pr.commitMessage, files }));
    } catch {
      throw err;
    }
  }
  const opened = await git.openPullRequest({ base: record.baseRef, head: branch, title: pr.title, body: pr.body, shas: [] });
  return { mode, sha, branch, prUrl: opened.url, files: paths, locked: true, at: opts.now() };
}
