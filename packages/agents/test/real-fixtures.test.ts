import { readdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ClaudeStreamParser, CodexStreamParser } from '../src/index.js';

// Drop transcripts captured from the real CLIs (docs/real-env/agents.md) into test/fixtures/ as
// real-claude-*.jsonl / real-codex-*.jsonl; they are checked here with no further test code.
const dir = fileURLToPath(new URL('./fixtures/', import.meta.url));
const real = readdirSync(dir).filter((f) => /^real-(claude|codex)-.*\.jsonl$/.test(f));

describe.skipIf(real.length === 0)('transcripts captured from the real CLIs', () => {
  it.each(real)('%s parses completely', (file) => {
    const text = readFileSync(`${dir}${file}`, 'utf8');
    if (file.startsWith('real-claude-')) {
      const p = new ClaudeStreamParser(() => 0);
      const events = [...p.push(text), ...p.end()];
      expect(p.sessionId, 'session id').toBeTruthy();
      expect(p.result, 'result line').toBeDefined();
      expect(events.some((e) => e.type === 'usage')).toBe(true);
      expect([...p.unknownTypes], 'unknown event types: review them').toEqual([]);
    } else {
      const p = new CodexStreamParser(() => 0);
      const events = [...p.push(text), ...p.end()];
      expect(p.threadId, 'thread id').toBeTruthy();
      expect(p.turn.completed || p.turn.failed).toBe(true);
      expect(events.length).toBeGreaterThan(0);
      expect([...p.unknownTypes], 'unknown event types: review them').toEqual([]);
    }
  });
});
