import { spawn } from 'node:child_process';

export interface SpawnOptions {
  cwd: string;
  env: Record<string, string>;
}

export interface ProcessExit {
  code: number | null;
  signal: string | null;
}

/** The slice of a child process the adapters use; real or fake. */
export interface ProcessHandle {
  onStdout(cb: (chunk: string) => void): void;
  onStderr(cb: (chunk: string) => void): void;
  writeStdin(data: string): void;
  endStdin(): void;
  /** Signal the whole process group (the CLI and every tool it started). */
  kill(signal?: NodeJS.Signals): void;
  /** Resolves once the process has exited and its stdio is drained. */
  exited: Promise<ProcessExit>;
}

export type Spawner = (command: string, args: string[], options: SpawnOptions) => ProcessHandle;

/** Spawn in a new process group (`detached`) so abort can kill the CLI and everything under it. */
export const nodeSpawner: Spawner = (command, args, { cwd, env }) => {
  const child = spawn(command, args, { cwd, env, detached: true, stdio: ['pipe', 'pipe', 'pipe'] });
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdin.on('error', () => {}); // EPIPE when the child exits before reading its prompt
  const exited = new Promise<ProcessExit>((resolve) => {
    let spawnError: Error | undefined;
    child.on('error', (err) => {
      spawnError = err;
      resolve({ code: null, signal: `spawn-error: ${err.message}` });
    });
    child.on('close', (code, signal) => {
      if (!spawnError) resolve({ code, signal });
    });
  });
  return {
    onStdout: (cb) => child.stdout.on('data', cb),
    onStderr: (cb) => child.stderr.on('data', cb),
    writeStdin: (data) => void child.stdin.write(data),
    endStdin: () => void child.stdin.end(),
    kill(signal = 'SIGTERM') {
      if (child.pid === undefined) return;
      try {
        process.kill(-child.pid, signal);
      } catch {
        try {
          child.kill(signal);
        } catch {
          /* already gone */
        }
      }
    },
    exited,
  };
};

/** Splits a chunked stream into lines; a trailing partial line is held until its newline (or `flush`). */
export class LineSplitter {
  private rest = '';
  push(chunk: string): string[] {
    this.rest += chunk;
    const lines = this.rest.split('\n');
    this.rest = lines.pop() ?? '';
    return lines.map((l) => (l.endsWith('\r') ? l.slice(0, -1) : l)).filter((l) => l.trim() !== '');
  }
  flush(): string[] {
    const last = this.rest.trim();
    this.rest = '';
    return last ? [last] : [];
  }
}

export interface RunProcessOptions extends SpawnOptions {
  stdin?: string;
  signal?: AbortSignal;
  /** Grace between SIGTERM and SIGKILL on abort. */
  killGraceMs?: number;
  onStdoutLine?(line: string): void;
}

export interface RunProcessResult extends ProcessExit {
  aborted: boolean;
  /** Last ~4 KB of stderr, for error messages. */
  stderrTail: string;
}

const STDERR_TAIL = 4096;

export async function runProcess(
  spawner: Spawner,
  command: string,
  args: string[],
  options: RunProcessOptions,
): Promise<RunProcessResult> {
  const { stdin, signal, killGraceMs = 5000, onStdoutLine, ...spawnOptions } = options;
  const child = spawner(command, args, spawnOptions);
  const lines = new LineSplitter();
  let stderrTail = '';
  child.onStdout((chunk) => {
    for (const line of lines.push(chunk)) onStdoutLine?.(line);
  });
  child.onStderr((chunk) => {
    stderrTail = (stderrTail + chunk).slice(-STDERR_TAIL);
  });
  if (stdin !== undefined) child.writeStdin(stdin);
  child.endStdin();

  let aborted = false;
  let killTimer: NodeJS.Timeout | undefined;
  const onAbort = () => {
    aborted = true;
    child.kill('SIGTERM');
    killTimer = setTimeout(() => child.kill('SIGKILL'), killGraceMs);
  };
  if (signal?.aborted) onAbort();
  else signal?.addEventListener('abort', onAbort, { once: true });

  try {
    const exit = await child.exited;
    for (const line of lines.flush()) onStdoutLine?.(line);
    return { ...exit, aborted, stderrTail: stderrTail.trim() };
  } finally {
    clearTimeout(killTimer);
    signal?.removeEventListener('abort', onAbort);
  }
}
