// `pnpm --filter @ratchet/e2e test:e2e`: build the desktop app, then run every spec under Xvfb.
//   SKIP_BUILD=1   reuse the existing packages/desktop/out
//   E2E_FILTER=x   only run spec files whose name contains x
import { spawnSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const run = (cmd, args, opts = {}) => {
  const r = spawnSync(cmd, args, { stdio: 'inherit', cwd: root, ...opts });
  if (r.status !== 0) process.exit(r.status ?? 1);
};

if (!process.env.SKIP_BUILD) run('pnpm', ['--filter', '@ratchet/desktop', 'build']);

const filter = process.env.E2E_FILTER ?? '';
const specs = readdirSync(join(root, 'specs'))
  .filter((f) => f.endsWith('.spec.mjs') && f.includes(filter))
  .sort()
  .map((f) => join('specs', f));

// A tall virtual screen so the window can be grown to the full content height for full-page screenshots.
const xvfb = ['-a', '-s', '-screen 0 1920x5200x24 -nolisten tcp'];
const useXvfb = !process.env.DISPLAY;
const test = [process.execPath, '--test', '--test-concurrency=1', '--test-timeout=1500000', ...specs];
if (useXvfb) run('xvfb-run', [...xvfb, ...test]);
else run(test[0], test.slice(1));
