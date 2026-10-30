/** Infrastructure failure (checkout, launch, misconfiguration), as opposed to a candidate failing a gate. */
export class BenchError extends Error {
  constructor(message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = 'BenchError';
  }
}

/** The job was aborted through its AbortSignal. `name` is `AbortError` so generic abort handling works. */
export class BenchAbortedError extends Error {
  constructor(message = 'bench job aborted') {
    super(message);
    this.name = 'AbortError';
  }
}

/** A queued or running job was cancelled through the BenchQueue. */
export class BenchCancelledError extends Error {
  constructor(message = 'bench job cancelled') {
    super(message);
    this.name = 'AbortError';
  }
}
