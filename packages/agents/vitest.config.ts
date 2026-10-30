import { defineConfig } from 'vitest/config';

// These tests drive real git repos and child processes; on a loaded machine (the full monorepo run) a few seconds
// is not enough, so the default 5 s timeout produced spurious failures.
export default defineConfig({
  test: {
    testTimeout: 30_000,
    hookTimeout: 30_000,
  },
});
