/**
 * Regenerate tests/golden/*.out from the built CLI:  pnpm golden
 * Only for deliberate output changes: the golden files are protected, a candidate cannot do this.
 */
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { GOLDEN_CASES } from './golden-cases.js';
import { CLI_PATH } from './mix.js';
import { GOLDEN_SPEC, buildWorkspace, cliEnv } from './workspace.js';

const dir = fileURLToPath(new URL('../../tests/golden/', import.meta.url));
mkdirSync(dir, { recursive: true });
const ws = buildWorkspace(GOLDEN_SPEC);
try {
  for (const c of GOLDEN_CASES) {
    const r = spawnSync(process.execPath, [CLI_PATH, ...c.args], { cwd: ws.cwd, env: cliEnv(ws), encoding: 'utf8' });
    if (r.status !== c.exit) throw new Error(`${c.name}: exit ${r.status}, expected ${c.exit}\n${r.stderr}`);
    writeFileSync(`${dir}${c.name}.out`, r[c.stream]);
  }
  console.log(`wrote ${GOLDEN_CASES.length} golden files to tests/golden/`);
} finally {
  ws.cleanup();
}
