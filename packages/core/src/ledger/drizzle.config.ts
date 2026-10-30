import { defineConfig } from 'drizzle-kit';

// Run from packages/core:
//   pnpm exec drizzle-kit generate --config src/ledger/drizzle.config.ts
// Paths below are relative to the cwd (packages/core).
export default defineConfig({
  dialect: 'sqlite',
  schema: './src/ledger/schema.ts',
  out: './src/ledger/migrations',
});
