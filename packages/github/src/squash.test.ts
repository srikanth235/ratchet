import { describe, expect, it } from 'vitest';
import { buildSquashPlan } from './index.js';

describe('buildSquashPlan', () => {
  it('builds message, trailers and sha list in ratchet order', () => {
    const plan = buildSquashPlan({
      metric: 'http_req_duration.p95',
      direction: 'minimize',
      wins: [
        { sha: 'a'.repeat(40), agentKind: 'claude-code', message: 'Cache tokenizer\n\nbody', fromValue: 400, toValue: 300 },
        { sha: 'b'.repeat(40), agentKind: 'codex', message: 'Index postings', fromValue: 300, toValue: 200 },
        { sha: 'c'.repeat(40), agentKind: 'claude-code', message: 'Pool buffers', fromValue: 200, toValue: 180 },
      ],
    });
    expect(plan.shas).toEqual(['a'.repeat(40), 'b'.repeat(40), 'c'.repeat(40)]);
    expect(plan.finalSha).toBe('c'.repeat(40));
    expect(plan.message).toMatchInlineSnapshot(`
      "ratchet: http_req_duration.p95 400 → 180 (55.0%, 3 verified wins)

      - Cache tokenizer (claude-code, aaaaaaaa, 25.0%)
      - Index postings (codex, bbbbbbbb, 33.3%)
      - Pool buffers (claude-code, cccccccc, 10.0%)

      Co-authored-by: Claude Code <noreply@anthropic.com>
      Co-authored-by: Codex <noreply@openai.com>"
    `);
    expect(plan.coAuthors).toHaveLength(2);
  });

  it('rejects an empty win list', () => {
    expect(() => buildSquashPlan({ metric: 'm', direction: 'minimize', wins: [] })).toThrow(/no verified wins/);
  });
});
