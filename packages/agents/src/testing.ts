import type { ProcessExit, ProcessHandle, SpawnOptions, Spawner } from './process.js';

export interface FakeProcessSpec {
  /** Full stdout text, delivered in `chunkSize` pieces (default: one chunk per line) after stdin closes. */
  stdout?: string;
  stderr?: string;
  exitCode?: number;
  chunkSize?: number;
  /** Never exits on its own; exits with the signal when killed (simulates a long-running agent turn). */
  hang?: boolean;
  /** Spawn fails (ENOENT-style). */
  spawnError?: string;
}

export interface FakeCall {
  command: string;
  args: string[];
  options: SpawnOptions;
  stdin: string;
  signals: NodeJS.Signals[];
}

/** A `Spawner` that replays canned output; for adapter tests and simulation. */
export class FakeSpawner {
  readonly calls: FakeCall[] = [];
  constructor(private readonly plan: FakeProcessSpec | ((call: FakeCall, index: number) => FakeProcessSpec)) {}

  readonly spawner: Spawner = (command, args, options) => {
    const call: FakeCall = { command, args, options, stdin: '', signals: [] };
    const index = this.calls.push(call) - 1;
    const spec = typeof this.plan === 'function' ? this.plan(call, index) : this.plan;
    const out: ((c: string) => void)[] = [];
    const err: ((c: string) => void)[] = [];
    let resolveExit!: (e: ProcessExit) => void;
    const exited = new Promise<ProcessExit>((r) => (resolveExit = r));
    let finished = false;
    const finish = (e: ProcessExit) => {
      if (finished) return;
      finished = true;
      resolveExit(e);
    };
    const play = () => {
      if (spec.spawnError) return finish({ code: null, signal: `spawn-error: ${spec.spawnError}` });
      const text = spec.stdout ?? '';
      const pieces = spec.chunkSize ? text.match(new RegExp(`[\\s\\S]{1,${spec.chunkSize}}`, 'g')) ?? [] : text.split(/(?<=\n)/).filter(Boolean);
      setImmediate(() => {
        for (const piece of pieces) out.forEach((cb) => cb(piece));
        if (spec.stderr) err.forEach((cb) => cb(spec.stderr as string));
        if (!spec.hang) finish({ code: spec.exitCode ?? 0, signal: null });
      });
    };
    return {
      onStdout: (cb) => void out.push(cb),
      onStderr: (cb) => void err.push(cb),
      writeStdin: (data) => void (call.stdin += data),
      endStdin: play,
      kill: (signal = 'SIGTERM') => {
        call.signals.push(signal);
        if (spec.hang) finish({ code: null, signal });
      },
      exited,
    } satisfies ProcessHandle;
  };
}
