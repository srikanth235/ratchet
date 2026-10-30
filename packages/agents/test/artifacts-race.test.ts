import { describe, expect, it } from 'vitest';
import type { ArtifactStore } from '@ratchet/core';
import { ArtifactInbox, eventKey, inboxKey, readEvents } from '../src/artifacts.js';

/** A store where a listed object reads as empty the first time: a writer that has created the file but not filled it. */
function racyStore(): ArtifactStore & { files: Map<string, string>; emptyOnce: Set<string> } {
  const files = new Map<string, string>();
  const emptyOnce = new Set<string>();
  return {
    files,
    emptyOnce,
    async put(key, body) {
      files.set(key, typeof body === 'string' ? body : new TextDecoder().decode(body));
    },
    async get(key) {
      if (emptyOnce.delete(key)) return '';
      return files.get(key) ?? null;
    },
    async list(prefix) {
      return [...files.keys()].filter((k) => k.startsWith(prefix)).sort();
    },
  };
}

describe('readers never skip an object that was listed before it was readable', () => {
  it('readEvents stops before an empty batch and returns it on the next poll', async () => {
    const store = racyStore();
    await store.put(eventKey('p/', 1), '{"type":"status","text":"one"}\n');
    await store.put(eventKey('p/', 2), '{"type":"commit","text":"two"}\n');
    store.emptyOnce.add(eventKey('p/', 2));
    const first = await readEvents(store, 'p/');
    expect(first.events.map((e) => e.text)).toEqual(['one']);
    expect(first.cursor).toBe('1');
    const second = await readEvents(store, 'p/', first.cursor);
    expect(second.events.map((e) => e.text)).toEqual(['two']); // the commit was not lost
  });

  it('the inbox retries a message that read as empty instead of dropping it', async () => {
    const store = racyStore();
    await store.put(inboxKey('p/', 1), JSON.stringify({ type: 'verdict', sha: 'abc', verdict: 'verified', reasons: [], currentBest: 1 }));
    store.emptyOnce.add(inboxKey('p/', 1));
    // Bounded: a reader that dropped the message would otherwise poll forever.
    const ac = new AbortController();
    let sleeps = 0;
    const clock = { now: () => 0, sleep: async () => void (++sleeps > 20 && ac.abort()) };
    const inbox = new ArtifactInbox(store, 'p/', clock as never, 1);
    expect(await inbox.next(ac.signal)).toMatchObject({ type: 'verdict', sha: 'abc' });
  });
});
