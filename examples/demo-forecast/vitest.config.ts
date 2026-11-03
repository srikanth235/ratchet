import { defineConfig } from 'vitest/config';

// The suites are quick, but the bench runs them while other jobs load the machine: keep slow CPUs from failing on timeouts.
export default defineConfig({ test: { testTimeout: 30_000 } });
