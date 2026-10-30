import type { Clock, CloudCompute, InstanceInfo, RemoteCommandResult } from '@ratchet/core';
import { BenchAbortedError, BenchError } from './errors.js';
import type { BenchLog } from './log.js';
import { pingScript, stripSentinels, wrapStep } from './remote-scripts.js';

export interface SessionOptions {
  compute: CloudCompute;
  clock: Clock;
  /** The first command is retried while the instance's SSM agent registers. Defaults: 30 attempts, 10 s apart. */
  readyAttempts?: number | undefined;
  readyDelayMs?: number | undefined;
  /** Called with non-fatal problems, e.g. a failed `terminate` (the reaper is the backstop). */
  onWarning?: ((message: string) => void) | undefined;
}

/**
 * One launched instance and the plumbing every remote job needs: wait for SSM, run a script raced against the abort signal
 * (output minus sentinel lines goes to the log) and terminate. Shared by the bench executor and the dependency-snapshot job,
 * which therefore have the same terminate-in-`finally` discipline.
 */
export class InstanceSession {
  constructor(
    private readonly o: SessionOptions,
    readonly instance: InstanceInfo,
    private readonly log: BenchLog,
    private readonly signal: AbortSignal | undefined,
    /** Used in log lines: `bench`, `dependency snapshot`. */
    private readonly label = 'bench',
  ) {}

  async waitReady(): Promise<void> {
    const attempts = this.o.readyAttempts ?? 30;
    for (let i = 1; ; i++) {
      try {
        const r = await this.call('ping', pingScript(), 60_000);
        if (r.exitCode === 0) return;
        throw new Error(`exit ${r.exitCode}`);
      } catch (err) {
        if (err instanceof BenchAbortedError) throw err;
        if (i >= attempts) throw new BenchError(`${this.label} instance ${this.instance.id} never became ready: ${(err as Error).message}`);
        await this.o.clock.sleep(this.o.readyDelayMs ?? 10_000, this.signal);
      }
    }
  }

  /** One runCommand, raced against the abort signal; the output (minus sentinel lines) goes to the log. */
  async call(name: string, script: string, timeoutMs: number): Promise<RemoteCommandResult> {
    if (this.signal?.aborted) throw new BenchAbortedError();
    const pending = this.o.compute.runCommand(this.instance.id, {
      script: wrapStep(script),
      timeoutSeconds: Math.ceil(timeoutMs / 1000) + 120,
    });
    pending.catch(() => undefined);
    let onAbort: (() => void) | undefined;
    const aborted = new Promise<never>((_, reject) => {
      if (!this.signal) return;
      onAbort = () => reject(new BenchAbortedError());
      this.signal.addEventListener('abort', onAbort, { once: true });
    });
    let res: RemoteCommandResult;
    try {
      res = await Promise.race([pending, aborted]);
    } catch (err) {
      if (err instanceof BenchAbortedError) throw err;
      throw new BenchError(`runCommand "${name}" on ${this.instance.id} failed: ${(err as Error).message}`, { cause: err });
    } finally {
      if (onAbort) this.signal?.removeEventListener('abort', onAbort);
    }
    if (name !== 'ping') {
      this.log.section(`remote step: ${name} (exit ${res.exitCode})`);
      this.log.line(stripSentinels(res.stdout));
      if (res.stderr.trim()) this.log.line(`[stderr]\n${res.stderr}`);
    }
    return res;
  }

  /** Terminates the instance; a failure is logged and reported, never thrown (the reaper cleans up what slips through). */
  async terminate(): Promise<void> {
    try {
      await this.o.compute.terminate([this.instance.id]);
      this.log.line(`${this.label} instance ${this.instance.id} terminated`);
    } catch (err) {
      const msg = `FAILED to terminate ${this.label} instance ${this.instance.id}: ${(err as Error).message} (the reaper must clean it up)`;
      this.log.line(msg);
      this.o.onWarning?.(msg);
    }
  }
}

export function tail(s: string, n = 600): string {
  const t = s.trim();
  return t.length > n ? '...' + t.slice(-n) : t;
}
