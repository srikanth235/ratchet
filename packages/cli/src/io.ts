/** Output sinks, injectable so commands can be tested without touching the process streams. */
export interface Io {
  out(text: string): void;
  err(text: string): void;
}

export const processIo: Io = {
  out: (t) => void process.stdout.write(t),
  err: (t) => void process.stderr.write(t),
};

/** An Io that records everything written to it. */
export function bufferIo(): Io & { stdout: string; stderr: string } {
  const b = {
    stdout: '',
    stderr: '',
    out(t: string) {
      b.stdout += t;
    },
    err(t: string) {
      b.stderr += t;
    },
  };
  return b;
}
