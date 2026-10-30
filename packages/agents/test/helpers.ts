import { readFileSync } from 'node:fs';
import type { AgentEvent } from '@ratchet/core';

export const fixture = (name: string): string =>
  readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8');

export function collect(): { events: AgentEvent[]; onEvent: (e: AgentEvent) => void } {
  const events: AgentEvent[] = [];
  return { events, onEvent: (e) => events.push(e) };
}
