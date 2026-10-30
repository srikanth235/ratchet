// Copy non-TS assets (SQL migrations) into dist so the packaged app can open the ledger.
import { cpSync } from 'node:fs';
cpSync('src/ledger/migrations', 'dist/ledger/migrations', { recursive: true });
