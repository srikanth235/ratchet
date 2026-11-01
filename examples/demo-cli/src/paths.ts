import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** The package root, whether running from `src/` (tests) or from `dist/src/` (built). */
const candidates = [new URL('../', import.meta.url), new URL('../../', import.meta.url)];
export const PACKAGE_ROOT = fileURLToPath(candidates.find((u) => existsSync(new URL('package.json', u))) ?? candidates[0]!);
export const MESSAGES_DIR = join(PACKAGE_ROOT, 'data', 'messages');
