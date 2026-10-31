// Fails when src/api.d.ts is not what `openapi-typescript` produces from api/openapi.yaml today.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const dir = mkdtempSync(join(tmpdir(), 'ratchet-contract-'));
try {
  const out = join(dir, 'api.d.ts');
  const run = spawnSync('pnpm', ['exec', 'openapi-typescript', '../../api/openapi.yaml', '-o', out], { stdio: 'inherit' });
  if (run.status !== 0) process.exit(run.status ?? 1);
  if (readFileSync(out, 'utf8') !== readFileSync('src/api.d.ts', 'utf8')) {
    console.error('packages/contract/src/api.d.ts is stale. Run `pnpm contract:gen` (and commit the result).');
    process.exit(1);
  }
} finally {
  rmSync(dir, { recursive: true, force: true });
}
