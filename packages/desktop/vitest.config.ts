import { resolve } from 'node:path';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  resolve: {
    alias: [
      { find: '@ratchet/core', replacement: resolve(__dirname, '../core/src/index.ts') },
      { find: '@ratchet/ui/tokens.css', replacement: resolve(__dirname, '../ui/src/tokens.css') },
      { find: '@ratchet/ui', replacement: resolve(__dirname, '../ui/src/index.ts') },
    ],
  },
  // Renderer tests drive the real service (and the dev engine's git repo): give them room on a loaded machine.
  test: { include: ['src/**/*.test.{ts,tsx}'], testTimeout: 30_000 },
});
