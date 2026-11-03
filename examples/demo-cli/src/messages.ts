import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { compileMessage, type Message, type Vars } from './icu.js';
import { MESSAGES_DIR } from './paths.js';

/**
 * Message catalogs: one JSON file per area under data/messages/ (`list.json`, `errors.json`, ...), keys
 * namespaced by the file name (`list.header.id`). Every catalog is read and compiled when the module loads.
 */
const catalog = new Map<string, Message>();

for (const file of readdirSync(MESSAGES_DIR).sort()) {
  if (!file.endsWith('.json')) continue;
  const entries = JSON.parse(readFileSync(join(MESSAGES_DIR, file), 'utf8')) as Record<string, string>;
  for (const [key, source] of Object.entries(entries)) catalog.set(key, compileMessage(source));
}

/** Format a message by key; unknown keys come back as `[key]` so a missing string is visible, not fatal. */
export function t(key: string, vars?: Vars): string {
  const m = catalog.get(key);
  return m ? m(vars) : `[${key}]`;
}

export function hasMessage(key: string): boolean {
  return catalog.has(key);
}

export function messageCount(): number {
  return catalog.size;
}
