# Glossary

| Term | Means | Not |
|---|---|---|
| Optimization | A goal Ratchet works on, with its schedule, limits and agents | automation, job, task |
| Run | One execution of an optimization, numbered `#14` | attempt, session |
| Delivered | Yearly value of merged wins | locked in, realised |
| Pending | Yearly value of wins in an open PR; not counted in Return | unclaimed, waiting |
| Return | Delivered ÷ spent, as `68×` | ROI |
| Agent | A Claude Code, Codex or Grok login or key that proposes changes | harness (code only), connection |
| Agent order | The fallback sequence, "Claude Code → Codex → Grok · on this Mac" | priority |
| Where | This Mac or Cloud: where agents run | location |
| Note | Text left on a run to steer the next candidate | comment, prompt |
| Pause Schedule | Stop new runs of an optimization | Pause |
| Pause Agent | Stop one agent inside a run | |
| Chat | A conversation with your own agent (Claude Code, Codex or Grok) that acts on Ratchet through its tools | assistant session, thread |
| Draft | An optimization the chat proposed as a card, not created yet | proposal |
| Config branch | `ratchet/config`, where a new optimization's files go before any PR | draft PR |
| Customize | Templates, Connectors, Agents: their only home | Settings |
| Default | A value for new optimizations, set in Settings → Defaults | global |
