// Copy non-TS assets (SQL migrations) into dist so the packaged app can open the ledger.
// The target is cleared first so a removed migration never lingers in dist.
import { cpSync, rmSync } from 'node:fs';
rmSync('dist/ledger/migrations', { recursive: true, force: true });
cpSync('src/ledger/migrations', 'dist/ledger/migrations', { recursive: true });
