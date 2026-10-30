import { defineConfig } from 'vitest/config';

// The contract suite spawns the built CLI; the bench runs it while other jobs load the machine.
export default defineConfig({ test: { testTimeout: 60_000, hookTimeout: 60_000 } });
