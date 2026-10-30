import { spawn } from 'node:child_process';

export interface ProcessOptions {
  cwd: string;
  /** Merged over `process.env`. */
  env?: Record<string, string | undefined>;
  /** Kill the whole process group after this many ms. */
  timeoutMs?: number;
  /** Kill the whole process group when aborted. */
  signal?: AbortSignal;
  /** Called with every chunk of output (stdout and stderr), e.g. to feed a log. */
  onData?: (chunk: string, stream: 'stdout' | 'stderr') => void;
  /** Keep at most this many trailing characters of stdout in `ProcessResult.stdout`. Default 8 Mi. */
  maxCaptureChars?: number;
}

export interface ProcessResult {
  exitCode: number | null;
  signal: NodeJS.Signals | null;
  /** The (tail of the) stdout; stderr is only delivered through `onData`. */
  stdout: string;
  timedOut: boolean;
  aborted: boolean;
  durationMs: number;
  /** Set when the process could not be spawned at all. */
  spawnError?: string;
}

/**
 * Run `command` with `bash -c` in its own process group, so a timeout or abort kills the shell AND
 * everything it spawned (bench server, load generator, ...). Never rejects. After the shell exits
 * the group is killed again to clear stragglers that would otherwise outlive the step.
 */
export function runProcess(command: string, opts: ProcessOptions): Promise<ProcessResult> {
  const cap = opts.maxCaptureChars ?? 8 * 1024 * 1024;
  return new Promise((resolve) => {
    const t0 = performance.now();
    const base = { stdout: '', timedOut: false, aborted: false, exitCode: null, signal: null } as const;
    if (opts.signal?.aborted) {
      resolve({ ...base, aborted: true, durationMs: 0 });
      return;
    }
    let child;
    try {
      child = spawn('bash', ['-c', command], {
        cwd: opts.cwd,
        env: { ...process.env, ...opts.env } as NodeJS.ProcessEnv,
        stdio: ['ignore', 'pipe', 'pipe'],
        detached: true, // own process group
      });
    } catch (err) {
      resolve({ ...base, durationMs: 0, spawnError: (err as Error).message });
      return;
    }
    const pid = child.pid;
    let stdout = '';
    let timedOut = false;
    let aborted = false;
    let done = false;
    let exit: { code: number | null; signal: NodeJS.Signals | null } | undefined;
    let graceTimer: NodeJS.Timeout | undefined;

    const killGroup = () => {
      try {
        if (pid !== undefined) process.kill(-pid, 'SIGKILL');
        else child.kill('SIGKILL');
      } catch {
        // group already gone
      }
    };
    const timer = opts.timeoutMs === undefined ? undefined : setTimeout(() => ((timedOut = true), killGroup()), opts.timeoutMs);
    const onAbort = () => {
      aborted = true;
      killGroup();
    };
    opts.signal?.addEventListener('abort', onAbort, { once: true });

    const finish = (spawnError?: string) => {
      if (done) return;
      done = true;
      if (timer) clearTimeout(timer);
      if (graceTimer) clearTimeout(graceTimer);
      opts.signal?.removeEventListener('abort', onAbort);
      child.stdout?.destroy();
      child.stderr?.destroy();
      resolve({
        exitCode: exit?.code ?? null,
        signal: exit?.signal ?? null,
        stdout,
        timedOut,
        aborted,
        durationMs: performance.now() - t0,
        ...(spawnError ? { spawnError } : {}),
      });
    };

    child.stdout?.setEncoding('utf8').on('data', (d: string) => {
      stdout += d;
      if (stdout.length > cap * 2) stdout = stdout.slice(-cap);
      opts.onData?.(d, 'stdout');
    });
    child.stderr?.setEncoding('utf8').on('data', (d: string) => opts.onData?.(d, 'stderr'));
    child.on('error', (err) => finish(err.message));
    child.on('exit', (code, signal) => {
      exit = { code, signal };
      killGroup(); // stragglers (orphaned servers) must not outlive the step
      // 'close' follows once the pipes drain; do not wait forever for a grandchild holding them open
      graceTimer = setTimeout(() => finish(), 1000);
    });
    child.on('close', () => finish());
  }).then((r) => {
    const res = r as ProcessResult;
    if (res.stdout.length > cap) res.stdout = res.stdout.slice(-cap);
    return res;
  });
}
