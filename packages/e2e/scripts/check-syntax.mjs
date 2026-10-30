// `build` / `typecheck` / `test` for this package: the e2e code is plain ESM that uses the globally installed
// Playwright, so there is nothing to compile. Syntax-check every source file so the root pipeline stays meaningful.
import { spawnSync } from 'node:child_process';
import { readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const files = [];
const walk = (dir) => {
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules' || name === 'artifacts') continue;
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p);
    else if (/\.(mjs|js)$/.test(name)) files.push(p);
  }
};
walk(root);
let bad = 0;
for (const f of files) {
  const r = spawnSync(process.execPath, ['--check', f], { encoding: 'utf8' });
  if (r.status !== 0) {
    bad++;
    console.error(r.stderr);
  }
}
console.log(`@ratchet/e2e: ${files.length} files syntax-checked${bad ? `, ${bad} failed` : ''}`);
process.exit(bad ? 1 : 0);
