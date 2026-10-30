#!/usr/bin/env node
import { runnerMain } from '../runner-main.js';

const ac = new AbortController();
for (const sig of ['SIGTERM', 'SIGINT'] as const) process.on(sig, () => ac.abort());

runnerMain({ env: process.env, signal: ac.signal }).then(
  (code) => process.exit(code),
  (err: unknown) => {
    process.stderr.write(`ratchet-runner: ${err instanceof Error ? err.message : String(err)}\n`);
    process.exit(2);
  },
);
