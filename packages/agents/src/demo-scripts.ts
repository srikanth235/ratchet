import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { AgentScript, ScriptStep, StepCondition } from './scripted.js';

/** On-disk step: a `patch` step names a file in the script directory instead of carrying the diff. */
type FileStep = Exclude<ScriptStep, { type: 'patch' }> | { type: 'patch'; name?: string; file: string; skip?: StepCondition };
interface FileScript {
  name: string;
  attempts: { name: string; steps: FileStep[] }[];
}

export const DEMO_SEARCH_SCRIPT_DIR = fileURLToPath(new URL('../scripts/demo-search/', import.meta.url));

/** Read a script JSON and inline its patch files. Patches are relative to the demo-search root. */
export function loadScript(file: string, patchDir = file.replace(/[^/\\]+$/, '')): AgentScript {
  const raw = JSON.parse(readFileSync(file, 'utf8')) as FileScript;
  return {
    name: raw.name,
    attempts: raw.attempts.map((a) => ({
      name: a.name,
      steps: a.steps.map((s): ScriptStep => {
        if (s.type !== 'patch' || !('file' in s)) return s as ScriptStep;
        return { type: 'patch', diff: readFileSync(join(patchDir, s.file), 'utf8'), ...(s.name ? { name: s.name } : {}), ...(s.skip ? { skip: s.skip } : {}) };
      }),
    })),
  };
}

/**
 * The shipped demo-search scripts. `cc` and `cx` play the same six real fixes from examples/demo-search/SOLUTIONS.md
 * in different orders, interleaved with deliberate bad attempts (protected-path edit, contract break, noise-level no-op).
 */
export function loadDemoSearchScript(variant: 'cc' | 'cx', dir = DEMO_SEARCH_SCRIPT_DIR): AgentScript {
  return loadScript(join(dir, `${variant}.script.json`), dir);
}
