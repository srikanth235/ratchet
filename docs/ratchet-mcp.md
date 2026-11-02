# Ratchet MCP server (P3)

Code: `crates/ratchet-mcp` (protocol, tool sets, session plumbing), `crates/ratchetd/src/mcp` (tokens, `/v1/mcp/*`, tool bodies),
`crates/ratchet-agents/src/runner_mcp.rs` (cloud optimization set), `crates/ratchet-cli/src/{table,mcp}.rs` (owner mode).
Tests: `ratchet-mcp/tests/protocol.rs`, `ratchetd/tests/api/mcp.rs`, `ratchet-agents/tests/ratchet_mcp.rs`, `ratchet-cli/tests/cli/mcp.rs`.

## Three ways in

| Who | Command | Talks to | Tools |
|---|---|---|---|
| A chat session (harness CLI) | `ratchetd mcp --session <token>` | the daemon, `POST /v1/mcp/call` on the unix socket | chat set |
| A local optimization agent | `ratchetd mcp --session <token>` | the daemon | optimization set |
| A cloud optimization agent | `ratchet-runner mcp` | its inbox and artifact store, no daemon | optimization set |
| The owner's own client | `ratchet mcp` (`claude mcp add ratchet -- ratchet mcp`) | the daemon, as the CLI does | one tool per CLI command |

All four are newline-delimited JSON-RPC 2.0 on stdio. `initialize` answers with the client's protocol version when it is one of
`2025-06-18`, `2025-03-26`, `2024-11-05`, else with `2025-06-18`. Stdout carries JSON-RPC only; logs go to stderr.

## Session tokens

`McpHub::sessions.issue(set, scope)` (in `ratchetd::mcp`) returns `{id, token, set}`; `revoke(id)` ends it. The chat engine (P4) issues
one when a chat opens and revokes it on close; P6 does the same for local optimization agents. A token

- opens `/v1/mcp/session` and `/v1/mcp/call` and nothing else (every other route answers 401), and the owner's bearer token does
  not open those two;
- is bound to one tool set. The child process lists that set, and the daemon checks the set again on every call;
- may be bound to a repo (a chat may only draft for it) or an optimization (a steering chat, or an agent's own optimization;
  other optimizations are refused).

`ratchet_mcp::launch::ratchetd_server(...)` / `runner_server(...)` build the `McpServer` for `SessionSpec::mcp`; `attach` puts it
in a spec. Every adapter turns it into its CLI's flags (Claude `--mcp-config` + `--strict-mcp-config`, Codex `-c mcp_servers.*` or
an overlay `config.toml`, Grok `session/new` or a per-session `config.toml`). `ratchetd mcp` also reads the token from
`RATCHET_MCP_SESSION` when `--session` is missing.

## Chat set (daemon)

`list_optimizations`, `get_optimization`, `get_run`, `get_overview`, `list_templates`, `list_connectors`, `list_repos`,
`draft_optimization`, `create_optimization`, `run_now`, `pause`, `resume`, `add_steering_note`, `open_pr`, `explain_result`.

- A run is identified by its optimization id (`get_run {run}`, `explain_result {run}`).
- `draft_optimization` renders the template (default `test-suite-time`) with the goal prepended to `RATCHET.md` and commits the files
  to the repo's **`ratchet/config`** branch: created from the default branch the first time, a new commit on top afterwards. It
  never writes to the base ref, opens no pull request and touches no other branch (`mcp::config_branch`). The card it returns
  names the draft, the commit and `baseRef: "ratchet/config"`.
- `create_optimization {draft_id}` creates the optimization from the draft (starting from `ratchet/config`) and starts it. There is
  no approval step: the user's chat message is the go-ahead.
- `pause` / `resume` act on every agent of the optimization; `add_steering_note` sends the note to every agent, which reads it at
  the start of its next turn.

## Optimization set

`steering_notes`, `history`, `connector_query`, `submit_candidate`.

| Tool | Daemon (local agents) | Runner (cloud agents) |
|---|---|---|
| `steering_notes` | the agent's `You: ...` log entries | `User` messages in `inbox/` since the last call |
| `history` | candidates and the ratchet series | verdict events from `events/` |
| `submit_candidate` | not yet (commit with the trailer) | writes `candidates/NNNNNNNN.json` |
| `connector_query` | not yet (no connector attached) | refused: runners never hold connector credentials |

Nothing in either set writes the evaluator, the protected set, budgets or other optimizations.

## Owner mode

`ratchet mcp` builds one tool per leaf command of the `ratchet` CLI from the clap definition (`ratchet_cli::table::command_table`),
so `--help` and the tools cannot drift. A tool call is turned back into `ratchet --json <command> ...` and run in-process; the token
is read from `ratchetd.token` on each call. The one hand-written table is `table::MARKS` (read-only / write / destructive); a test
fails when a command has no mark. Destructive tools (`run`, `stop`, `pr`, `env_end`) only answer with what they would do until the
call carries `confirm: true`, the MCP form of `--yes` (forwarded as `--yes` once the CLI has the flag). `events` is bounded (20
events, 10 s) unless the call says otherwise; `report` has no `out` argument (it returns the text).
