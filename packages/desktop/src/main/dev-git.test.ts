import { openLedger } from '@ratchet/core';
import { afterEach, describe, expect, it } from 'vitest';
import { createDevEngine } from './dev-app.js';
import { DEMO_RUNNING_ID } from './seed.js';
import { createService, type RatchetService } from './service.js';
import { SettingsStore } from './settings.js';

const cipher = { isAvailable: () => true, encrypt: (s: string) => Buffer.from(`enc:${s}`), decrypt: (b: Buffer) => b.toString().slice(4) };
let service: RatchetService | undefined;
afterEach(() => service?.dispose());

describe('winning change (dev engine: real demo patches committed onto examples/demo-search)', () => {
  it('diffs the best verified commit of the seeded search run: real files and line counts, a real hunk', async () => {
    const ledger = openLedger(':memory:');
    service = createService({ ledger, settings: new SettingsStore(null, cipher, {}), createApp: (m) => createDevEngine(ledger, m, { tickMs: 3_600_000 }) });
    const w = await service.winningChange(DEMO_RUNNING_ID);
    expect(w.unavailable).toBeNull();
    expect(w.commits).toBe(5);
    expect(w.files.map((f) => f.path).sort()).toEqual(['src/filter.ts', 'src/search.ts']);
    expect(w.additions).toBe(w.files.reduce((n, f) => n + f.additions, 0));
    expect(w.additions).toBeGreaterThan(20);
    expect(w.excerpt!.path).toBe('src/search.ts');
    expect(w.excerpt!.lines[0]!.kind).toBe('hunk');
    expect(w.excerpt!.lines.some((l) => l.kind === 'add')).toBe(true);
  }, 60_000);

  it('says why when there is nothing to diff: no win yet, or a repo the dev engine has no history for', async () => {
    const ledger = openLedger(':memory:');
    service = createService({ ledger, settings: new SettingsStore(null, cipher, {}), createApp: (m) => createDevEngine(ledger, m, { tickMs: 3_600_000 }) });
    expect(await service.winningChange('opt_demo_web')).toMatchObject({ commits: 0, unavailable: 'No verified win yet.' });
    const ingest = await service.winningChange('opt_demo_ingest');
    expect(ingest.commits).toBe(2);
    expect(ingest.unavailable).toMatch(/^No diff available for [0-9a-f]{7}\.$/);
  });
});
