import { appendFile, mkdir, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import type {
  AgentEvent,
  AgentSession,
  AgentUsage,
  CandidateVerdict,
  RunnerLoopOptions,
  RunnerMessage,
} from '@ratchet/core';
import { AgentAbortedError } from './cli-agent.js';
import { buildContext, buildInitialPrompt, buildResumeMessage, type HistoryEntry } from './context.js';
import { Git, hasCandidateTrailer, withCandidateTrailer, execGit, type GitRunner } from './git.js';

export type RunnerStopReason = 'stopped' | 'deadline' | 'budget' | 'idle' | 'aborted' | 'failed' | 'agent-unavailable';

export interface RunnerLoopDeps extends RunnerLoopOptions {
  /** Injectable git (default: the `git` CLI). */
  git?: GitRunner;
  /** Extra environment for git, e.g. credentials. */
  gitEnv?: Record<string, string>;
  /** Remote branch where the engine keeps the ratchet best. Default `ratchet/best`. */
  bestRef?: string;
  remote?: string;
  /** Consecutive turns without a change before the loop gives up. Default 2. */
  maxIdleTurns?: number;
  /** Consecutive crashed agent turns before the loop gives up. Default 3. */
  maxConsecutiveFailures?: number;
  /** Pause between a crashed turn and the retry. Default 5 s. */
  retryDelayMs?: number;
  pushRetries?: number;
  /** Base delay for push retries (doubles each attempt). Default 1 s. */
  pushRetryDelayMs?: number;
  /** For adapters that report tokens but no dollars. */
  estimateCost?(usage: AgentUsage): number;
  model?: string;
  maxTurns?: number;
  agentEnv?: Record<string, string>;
}

export interface RunnerLoopResult {
  reason: RunnerStopReason;
  turns: number;
  candidates: number;
  spendUsd: number;
  bestValue: number;
  history: HistoryEntry[];
}

type VerdictMessage = Extract<RunnerMessage, { type: 'verdict' }>;

/** Buffers inbox messages so a verdict that arrives early (or during a turn) is never lost. */
class Mailbox {
  readonly verdicts = new Map<string, VerdictMessage>();
  readonly userMessages: string[] = [];
  paused = false;
  stopped = false;
  bestSeen: number | undefined;
  private waiters = new Set<() => void>();
  private readonly ac = new AbortController();

  constructor(
    private readonly inbox: RunnerLoopOptions['inbox'],
    private readonly onStop: () => void,
    private readonly onEvent: (e: Omit<AgentEvent, 'at'>) => void,
  ) {
    void this.pump();
  }

  private async pump(): Promise<void> {
    while (!this.ac.signal.aborted) {
      let message: RunnerMessage | null;
      try {
        message = await this.inbox.next(this.ac.signal);
      } catch {
        return;
      }
      if (!message) return;
      this.apply(message);
      for (const w of [...this.waiters]) w();
    }
  }

  private apply(m: RunnerMessage): void {
    switch (m.type) {
      case 'verdict':
        if (m.verdict !== 'pending') this.verdicts.set(m.sha, m);
        this.bestSeen = m.currentBest;
        break;
      case 'user':
        this.userMessages.push(m.text);
        this.onEvent({ type: 'status', text: 'user message received', data: { text: m.text } });
        break;
      case 'pause':
        this.paused = true;
        break;
      case 'resume':
        this.paused = false;
        break;
      case 'stop':
        this.stopped = true;
        this.onStop();
        break;
    }
  }

  /** Resolves on the next inbox message, or when `signal` aborts. */
  changed(signal: AbortSignal): Promise<void> {
    return new Promise((resolve) => {
      if (signal.aborted) return resolve();
      const done = () => {
        this.waiters.delete(done);
        signal.removeEventListener('abort', done);
        resolve();
      };
      this.waiters.add(done);
      signal.addEventListener('abort', done, { once: true });
    });
  }

  close(): void {
    this.ac.abort();
    for (const w of [...this.waiters]) w();
  }
}

const firstLine = (s: string, max = 72): string => (s.split('\n')[0] ?? '').trim().slice(0, max);

/**
 * `ratchet runner`: owns one agent on one branch.
 * prompt → agent turn → commit (`Ratchet-Candidate: true`) → push → wait for verdict → sync to the
 * ratchet best → resume with the verdict → repeat until stop / deadline / budget / idle.
 */
export async function runRunnerLoop(opts: RunnerLoopDeps): Promise<RunnerLoopResult> {
  const { config, adapter, workdir, inbox, clock } = opts;
  const remote = opts.remote ?? 'origin';
  const bestBranch = opts.bestRef ?? 'ratchet/best';
  const bestTracking = `refs/remotes/${remote}/${bestBranch}`;
  const maxIdle = opts.maxIdleTurns ?? 2;
  const maxFailures = opts.maxConsecutiveFailures ?? 3;
  const git = new Git(workdir, opts.git ?? execGit, opts.gitEnv);

  const emit = (e: Omit<AgentEvent, 'at'>) => opts.emit({ at: clock.now(), ...e });
  const status = (text: string, data?: Record<string, unknown>) => emit({ type: 'status', text, ...(data ? { data } : {}) });

  // ---- control: one abort controller for stop / deadline / external signal; `reason` says why.
  const ctl = new AbortController();
  let reason: RunnerStopReason | undefined;
  const halt = (r: RunnerStopReason) => {
    reason ??= r;
    ctl.abort();
  };
  if (opts.signal?.aborted) halt('aborted');
  else opts.signal?.addEventListener('abort', () => halt('aborted'), { once: true });
  void (async () => {
    while (!ctl.signal.aborted && clock.now() < config.deadlineAt) await clock.sleep(Math.max(1, config.deadlineAt - clock.now()), ctl.signal);
    if (!ctl.signal.aborted) halt('deadline');
  })();
  const mailbox = new Mailbox(inbox, () => halt('stopped'), emit);

  const state = {
    best: config.baselineValue,
    spendUsd: 0,
    turns: 0,
    history: [] as HistoryEntry[],
    overBudget: false,
    lastMessage: '',
    bestSha: undefined as string | undefined,
  };
  const result = (): RunnerLoopResult => ({
    reason: reason ?? 'stopped',
    turns: state.turns,
    candidates: state.history.length,
    spendUsd: state.spendUsd,
    bestValue: state.best,
    history: state.history,
  });

  try {
    // ---- setup
    status(`runner started for ${config.agentId} on ${config.branch}`, { kind: adapter.kind, branch: config.branch });
    const identity = await git.try(['config', 'user.name']);
    if (identity.stdout.trim() === '') {
      await git.run(['config', 'user.name', `${config.agentId} (ratchet)`]);
      await git.run(['config', 'user.email', `${config.agentId}@ratchet.local`]);
    }
    const excludeFile = resolve(workdir, (await git.run(['rev-parse', '--git-path', 'info/exclude'])).trim());
    await mkdir(dirname(excludeFile), { recursive: true });
    await appendFile(excludeFile, '\n.ratchet/\n');
    const baseHead = await git.head();
    await git.run(['update-ref', 'refs/ratchet/accepted', baseHead]);
    const known = new Set<string>([baseHead]);

    const preflight = await adapter.preflight();
    if (!preflight.ok) {
      emit({ type: 'error', text: `agent preflight failed: ${preflight.problems.join('; ')}`, data: { problems: preflight.problems } });
      reason = 'agent-unavailable';
      return result();
    }

    const writeContext = async (turn: number): Promise<string> => {
      const text = buildContext({ config, bestValue: state.best, history: state.history, now: clock.now(), spendUsd: state.spendUsd, turn, bestSha: state.bestSha?.slice(0, 8) });
      await mkdir(join(workdir, '.ratchet'), { recursive: true });
      await writeFile(join(workdir, '.ratchet', 'context.md'), text);
      return text;
    };

    const onAgentEvent = (e: AgentEvent) => {
      if (e.type === 'message' && e.text) state.lastMessage = e.text;
      if (e.type === 'usage' && e.usage) {
        state.spendUsd += e.usage.costUsd ?? opts.estimateCost?.(e.usage) ?? 0;
        if (state.spendUsd >= config.budgetUsd) state.overBudget = true;
      }
      opts.emit(e);
    };

    // ---- helpers
    const waitUnpaused = async (): Promise<void> => {
      if (!mailbox.paused) return;
      status('paused');
      while (mailbox.paused && !ctl.signal.aborted) await mailbox.changed(ctl.signal);
      if (!ctl.signal.aborted) status('resumed');
    };

    const commitIfChanged = async (turn: number): Promise<{ sha: string; files: string[]; subject: string } | null> => {
      await git.run(['add', '-A']);
      const staged = (await git.try(['diff', '--cached', '--quiet'])).code !== 0;
      if (staged) {
        const subject = firstLine(state.lastMessage) || `candidate ${turn}`;
        const body = state.lastMessage.trim() ? `\n\n${state.lastMessage.trim()}` : '';
        await git.run(['commit', '--quiet', '--no-verify', '-m', withCandidateTrailer(`ratchet: ${subject}${body}`)]);
      } else {
        const head = await git.head();
        if (known.has(head)) return null;
        if (!hasCandidateTrailer(await git.message())) {
          await git.run(['commit', '--quiet', '--amend', '--no-verify', '-m', withCandidateTrailer(await git.message())]);
        }
      }
      const sha = await git.head();
      if (known.has(sha)) return null;
      const parent = (await git.revParse('HEAD~1')) ?? sha;
      const accepted = (await git.revParse('refs/ratchet/accepted')) ?? parent;
      const files = await git.changedFiles(accepted, sha).catch(() => []);
      return { sha, files, subject: firstLine(await git.message(), 100) };
    };

    const push = async (): Promise<boolean> => {
      const attempts = (opts.pushRetries ?? 3) + 1;
      let last = '';
      for (let i = 0; i < attempts && !ctl.signal.aborted; i++) {
        const res = await git.try(['push', '--quiet', '--force-with-lease', remote, `HEAD:refs/heads/${config.branch}`]);
        if (res.code === 0) return true;
        last = res.stderr.trim();
        await clock.sleep((opts.pushRetryDelayMs ?? 1000) * 2 ** i, ctl.signal);
      }
      emit({ type: 'error', text: `push failed: ${last}`, data: { fatal: true } });
      return false;
    };

    /** Fetch the ratchet best and rebase onto it when it moved. Returns notes for the agent. */
    const syncToBest = async (): Promise<string[]> => {
      const fetched = await git.try(['fetch', '--quiet', remote, `+refs/heads/${bestBranch}:${bestTracking}`]);
      const bestSha = fetched.code === 0 ? await git.revParse(bestTracking) : null;
      if (!bestSha) return [];
      state.bestSha = bestSha;
      await git.run(['update-ref', 'refs/ratchet/accepted', bestSha]);
      if (await git.isAncestor(bestSha, 'HEAD')) return [];
      const before = await git.head();
      const mine = (await git.run(['rev-list', '--count', `${bestSha}..HEAD`])).trim();
      // `-X ours` in a rebase means the branch being rebased onto: conflicts resolve in favour of the best.
      const rebase = await git.try(['rebase', '--quiet', '-X', 'ours', bestSha]);
      if (rebase.code === 0) {
        status(`rebased onto ratchet best ${bestSha.slice(0, 8)}`, { from: before, to: await git.head(), commits: Number(mine) });
        return [`The ratchet best moved (now ${bestSha.slice(0, 8)}, set by an earlier verified candidate). Your ${mine} unverified commit(s) were rebased onto it; where they conflicted, the best's version was kept. Re-run the tests and re-measure before trusting your earlier numbers.`];
      }
      await git.try(['rebase', '--abort']);
      await git.run(['reset', '--hard', bestSha]);
      emit({ type: 'status', text: `rebase onto ${bestSha.slice(0, 8)} conflicted; reset to best`, data: { discarded: before } });
      return [`The ratchet best moved (now ${bestSha.slice(0, 8)}) and rebasing your ${mine} commit(s) onto it conflicted, so your branch was reset to the best and those commits (last: ${before.slice(0, 8)}) were discarded. Re-apply the idea on top of the new best if it still makes sense.`];
    };

    // ---- the loop
    let session: AgentSession | undefined;
    let message = '';
    let idleTurns = 0;
    let failures = 0;
    let notes: string[] = [];
    let entry: HistoryEntry | undefined;
    let noChange = false;

    while (!ctl.signal.aborted) {
      await waitUnpaused();
      if (ctl.signal.aborted) break;
      state.turns++;
      const contextText = await writeContext(state.turns);
      const spec = {
        workdir,
        prompt: '',
        ...(opts.model ? { model: opts.model } : {}),
        ...(opts.maxTurns ? { maxTurns: opts.maxTurns } : {}),
        ...(opts.agentEnv ? { env: opts.agentEnv } : {}),
      };

      // -- agent turn
      try {
        if (!session) {
          status('agent turn started', { turn: state.turns, resumed: false });
          session = await adapter.start({ ...spec, prompt: buildInitialPrompt(config, contextText) }, onAgentEvent, ctl.signal);
        } else {
          status('agent turn started', { turn: state.turns, resumed: true });
          message = buildResumeMessage({ entry, bestValue: state.best, config, now: clock.now(), spendUsd: state.spendUsd, notes, userMessages: mailbox.userMessages.splice(0), noChange });
          await adapter.resume(session, spec, message, onAgentEvent, ctl.signal);
        }
        failures = 0;
      } catch (err) {
        if (err instanceof AgentAbortedError || ctl.signal.aborted) {
          if (err instanceof AgentAbortedError && err.session && !session) session = err.session;
          break;
        }
        failures++;
        emit({ type: 'error', text: `agent turn failed: ${err instanceof Error ? err.message : String(err)}`, data: { failures } });
        if (failures >= maxFailures) {
          reason = 'failed';
          break;
        }
        await clock.sleep(opts.retryDelayMs ?? 5000, ctl.signal);
        // retry as a resume if we have a session, else restart; tell the agent what happened
        notes = [`Your previous turn crashed (${err instanceof Error ? err.message : String(err)}). Check the tree for half-finished edits and continue.`];
        entry = undefined;
        noChange = false;
        if (!session) state.turns--;
        continue;
      }

      // -- submit
      const candidate = await commitIfChanged(state.turns);
      notes = [];
      entry = undefined;
      noChange = false;
      if (!candidate) {
        idleTurns++;
        status('no change after agent turn', { idleTurns });
        if (idleTurns >= maxIdle && mailbox.userMessages.length === 0) {
          reason = 'idle';
          break;
        }
        noChange = true;
        notes = await syncToBest();
        known.add(await git.head());
        continue;
      }
      idleTurns = 0;
      if (!(await push())) {
        reason ??= 'failed';
        break;
      }
      known.add(candidate.sha);
      emit({ type: 'commit', text: candidate.subject, data: { sha: candidate.sha, branch: config.branch, files: candidate.files } });

      // -- wait for the verdict
      status('waiting for verdict', { sha: candidate.sha });
      while (!mailbox.verdicts.has(candidate.sha) && !ctl.signal.aborted) await mailbox.changed(ctl.signal);
      const verdict = mailbox.verdicts.get(candidate.sha);
      if (!verdict) break;
      mailbox.verdicts.delete(candidate.sha);
      state.best = verdict.currentBest;
      entry = { n: state.history.length + 1, sha: candidate.sha, verdict: verdict.verdict as CandidateVerdict, reasons: verdict.reasons, best: verdict.currentBest, at: clock.now() };
      state.history.push(entry);
      emit({ type: 'verdict', text: `${verdict.verdict}`, data: { sha: candidate.sha, verdict: verdict.verdict, reasons: verdict.reasons, currentBest: verdict.currentBest } });
      if (verdict.verdict === 'verified') await git.run(['update-ref', 'refs/ratchet/accepted', candidate.sha]);

      notes = await syncToBest();
      known.add(await git.head());
      if (state.overBudget) {
        reason = 'budget';
        break;
      }
    }
  } catch (err) {
    emit({ type: 'error', text: `runner crashed: ${err instanceof Error ? err.message : String(err)}`, data: { fatal: true } });
    reason ??= 'failed';
  } finally {
    const r = result();
    status(`runner finished: ${r.reason}`, { reason: r.reason, turns: r.turns, candidates: r.candidates, spendUsd: r.spendUsd });
    mailbox.close();
    ctl.abort();
  }
  return result();
}
