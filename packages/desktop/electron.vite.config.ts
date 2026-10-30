import { resolve } from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig, externalizeDepsPlugin } from 'electron-vite';

// Resolve @ratchet/ui from source so dev/build don't depend on the ui package being built first.
const uiAlias = [
  { find: '@ratchet/ui/tokens.css', replacement: resolve(__dirname, '../ui/src/tokens.css') },
  { find: '@ratchet/ui', replacement: resolve(__dirname, '../ui/src/index.ts') },
];

export default defineConfig({
  main: {
    plugins: [externalizeDepsPlugin()],
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/main/index.ts') } } },
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    build: { rollupOptions: { input: { index: resolve(__dirname, 'src/preload/index.ts') } } },
  },
  renderer: {
    root: resolve(__dirname, 'src/renderer'),
    resolve: { alias: uiAlias },
    plugins: [react()],
    build: {
      // Keep fonts as real files so the CSP can stay font-src 'self' (no data: URIs).
      assetsInlineLimit: 0,
      rollupOptions: { input: { index: resolve(__dirname, 'src/renderer/index.html') } },
    },
  },
});
