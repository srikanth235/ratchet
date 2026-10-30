import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import Database from 'better-sqlite3';
import { drizzle, type BetterSQLite3Database } from 'drizzle-orm/better-sqlite3';
import { migrate } from 'drizzle-orm/better-sqlite3/migrator';
import * as schema from './schema.js';
import { Ledger, type LedgerOptions } from './store.js';

export type LedgerDb = BetterSQLite3Database<typeof schema>;

export interface OpenLedgerOptions extends LedgerOptions {
  /** Override the folder holding drizzle SQL migrations. */
  migrationsFolder?: string;
}

/**
 * Locate the migrations folder. `tsc` does not copy .sql files, so when running
 * from `dist/` we fall back to the sources in `src/ledger/migrations`.
 */
export function resolveMigrationsFolder(): string {
  const candidates = [
    new URL('./migrations', import.meta.url),
    new URL('../../src/ledger/migrations', import.meta.url),
  ];
  for (const url of candidates) {
    const p = fileURLToPath(url);
    if (existsSync(p)) return p;
  }
  throw new Error('ratchet ledger: migrations folder not found');
}

/** Open (creating if needed) a ledger database and apply pending migrations. */
export function openLedger(path: string = ':memory:', options: OpenLedgerOptions = {}): Ledger {
  const sqlite = new Database(path);
  try {
    sqlite.pragma('foreign_keys = ON');
    if (path !== ':memory:') sqlite.pragma('journal_mode = WAL');
    sqlite.pragma('busy_timeout = 5000');
    const db: LedgerDb = drizzle(sqlite, { schema });
    migrate(db, { migrationsFolder: options.migrationsFolder ?? resolveMigrationsFolder() });
    return new Ledger(db, sqlite, options);
  } catch (err) {
    sqlite.close();
    throw err;
  }
}
