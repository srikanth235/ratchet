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
  test: { include: ['src/**/*.test.{ts,tsx}'] },
});
