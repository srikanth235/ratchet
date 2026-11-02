# Harness matrix (S0 result)

Output of phase S0 in `AGENT_HARNESS_PLAN.md`. It resolves every `?` cell of section 4.1 and adds the rate-limit and auth-expired
shapes and each provider's published terms.

**Legend.** ✓ verified against a real CLI in this session. ◐ from published docs or open source, with a mock fixture; it must be
confirmed during real-world testing (not approved yet), and its fallback stays wired. ✗ missing, with the fallback from the plan.

**What was and was not checked.**

- Claude Code `2.1.316` is installed here. Its flags were read from `claude --help`, `claude auth --help` and
  `claude auth status`. No Codex or Grok binary was installed or run.
- Codex and Grok facts come from docs and source fetched in this session, so they describe the current `main` and docs site, not a
  tagged `0.166.0` / `1.0.70`. Treat every ◐ cell as "documented for a nearby version".
- The Codex docs moved: `developers.openai.com/codex/*` answers with a 308 redirect to `learn.chatgpt.com/docs/*`. Links below use
  the `developers.openai.com` form.
- Grok Build is open source (Apache-2.0) at <https://github.com/xai-org/grok-build>; its user guide lives in
  `crates/codegen/xai-grok-pager/docs/user-guide/` (cited below as `grok-guide/NN-name.md`). The official installer is
  `curl -fsSL https://x.ai/cli/install.sh | bash` ([overview](https://docs.x.ai/build/overview)); an `@xai-official/grok` npm package
  named in the plan was **not confirmed** by any source found.

## 1. The `?` cells of section 4.1

Tally: **8 ◐, 2 ✗, 0 ✓** of the 10 `?` cells. Every other 4.1 cell was already ✓; the Claude ones are re-checked in section 2.

| Capability | Codex | Grok |
|---|---|---|
| Text deltas | ◐ **app-server only**: `item/agentMessage/delta` notification (also `item/reasoning/textDelta`, `item/reasoning/summaryTextDelta`). In `codex exec --json` the `agent_message` item arrives once, complete; `item.updated` exists in the event enum but the docs show it only for `todo_list`. So `exec` is ✗ and falls back to whole messages. Sources: [exec_events.rs](https://github.com/openai/codex/blob/main/codex-rs/exec/src/exec_events.rs), [app-server-protocol common.rs](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/common.rs), [app-server docs](https://developers.openai.com/codex/app-server). Fixture: `codex-0.166.0-mock-deltas.jsonl`. | (already ✓ in code) `text` lines in `--output-format streaming-json`. |
| Long-lived session | ◐ `codex app-server` (default `--listen stdio://`): JSONL JSON-RPC with the `"jsonrpc"` key omitted. `initialize` → `initialized` notification → `thread/start` / `thread/resume` → `turn/start` (response, then notifications until `turn/completed`) → `turn/interrupt {threadId, turnId}`. Also `turn/steer`. Source: [app-server docs](https://developers.openai.com/codex/app-server). Fixture: `codex-0.166.0-mock-session.jsonl`. | ◐ `grok agent stdio`: ACP over stdin/stdout, standard JSON-RPC 2.0. `initialize {protocolVersion:1}` → `authenticate {methodId: "xai.api_key" \| "cached_token", _meta:{headless:true}}` → `session/new {cwd, mcpServers}` → `session/prompt {sessionId, prompt:[{type:"text",text}]}`; text arrives as `session/update` with `update.sessionUpdate = "agent_message_chunk"` before the `session/prompt` response carrying `stopReason`. Also `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`; `session/set_config_option` changes model/effort; `session/cancel` is not documented. Sources: [headless-scripting](https://docs.x.ai/build/cli/headless-scripting), [grok-guide/15-agent-mode.md](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md). Fixture: `grok-1.0.70-mock-session.jsonl`. |
| Inject MCP servers | ◐ `-c mcp_servers.<name>.command="…"` / `.args=[…]` / `.env.K="v"`. `-c key=value` takes dotted keys with TOML values and repeats ([config-advanced](https://developers.openai.com/codex/config-advanced)); the MCP table schema is `[mcp_servers.<name>]` with `command`, `args`, `env`, `cwd`, `url`, `enabled_tools`, `disabled_tools`, `startup_timeout_sec` ([mcp docs](https://developers.openai.com/codex/mcp)). The docs give no `-c mcp_servers…` example, so the combination is inferred. App-server `thread/start` is said to accept "MCP overrides" but the parameter name is undocumented. `--strict-config` helps catch a typo. **Fallback:** temp `CODEX_HOME` overlay with a generated `config.toml`. Fixture: `codex-0.166.0-mock-mcp-tool.jsonl` (`mcp_tool_call` item: `server`, `tool`, `arguments`, `result`, `status`, per [exec_events.rs](https://github.com/openai/codex/blob/main/codex-rs/exec/src/exec_events.rs)). | ◐ **ACP only:** `session/new {mcpServers:[…]}` (documented). The one-shot `grok -p` has **no** documented per-invocation MCP flag. Config route: `[mcp_servers.<name>]` in `~/.grok/config.toml` or a project `.grok/config.toml` (project files contribute only `mcp_servers`, `plugins`, `permission`), fields `command`, `args`, `env`, `cwd`, `url`, `headers`, `startup_timeout_sec`. `GROK_CONFIG` (inline JSON) / `GROK_CONFIG_PATH` overlay "allowlisted keys only"; whether `mcp_servers` is allowlisted is not stated. Strictness: user MCP sources are switched off with `[compat.claude] mcps=false`, `[compat.cursor] mcps=false` (env `GROK_CLAUDE_MCPS_ENABLED`, `GROK_CURSOR_MCPS_ENABLED`); `.mcp.json` has no toggle. **Fallback:** per-session `.grok/config.toml` in the worktree plus a temp `GROK_HOME`. Sources: [mcp-servers](https://docs.x.ai/build/features/mcp-servers), [grok-guide/07](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/07-mcp-servers.md), [grok-guide/26](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/26-config-reference.md). Fixture: `grok-1.0.70-mock-mcp-tool.jsonl` (tool-name format `mcp__ratchet__…` is a guess; docs do not show it). |
| Read-only access | (already ✓) `sandbox_mode="read-only"`. App-server equivalents: `sandbox:"read-only"` on `thread/start`/`turn/start`, `SandboxPolicy {type:"read-only"}`. Fixture: `codex-0.166.0-mock-read-only-refusal.jsonl`. | ◐ `--sandbox read-only` (also `GROK_SANDBOX`, `[sandbox] profile`): writes denied except `~/.grok`, `/tmp`, `/var/tmp`; child-process network blocked on Linux only (seccomp), a no-op on macOS. Plus `--tools <allowlist>` / `--disallowed-tools` (removes tools; the denylist wins) and `--allow`/`--deny` rules (`Edit(...)`, `Bash(npm*)`). **Catch:** the ACP agent docs say leader mode is refused when a non-`off` sandbox is requested; read-only in `grok agent stdio` therefore needs the tool allowlist, not the sandbox. The error text a denied write produces is not documented. Sources: [grok-guide/18-sandbox.md](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/18-sandbox.md), [grok-guide/14-headless-mode.md](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/14-headless-mode.md). Fixture: `grok-1.0.70-mock-read-only-refusal.jsonl`. |
| System prompt add-on | ◐ `-c developer_instructions="…"` (appears in the sample config; its exact semantics are not described). `model_instructions_file` **replaces** the built-in instructions, so it is not an add-on; the plain `instructions` key is "reserved". Safest add-on is `AGENTS.md` in an overlay directory. Sources: [config-advanced](https://developers.openai.com/codex/config-advanced), [config-reference](https://developers.openai.com/codex/config-reference). | ◐ `--rules <TEXT>` ("custom system prompt rules", headless), plus Claude-compatible aliases `--append-system-prompt` / `--system-prompt`; `--system-prompt-override` replaces. ACP: `session/new` `_meta.rules` appends, `_meta.systemPromptOverride` replaces. Sources: [grok CLI reference](https://docs.x.ai/build/cli/reference), [grok-guide/15](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md). |
| Built-in model fallback | ✗ No key or flag documented in the CLI reference, config reference or exec docs. **Fallback:** Ratchet's own harness/model fallback (plan 4.6). | ✗ None documented ([grok-guide/26](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/26-config-reference.md)). **Fallback:** same. |

### Mismatches with the code that P1 must handle

- **Grok `-p -`.** `grok.rs` builds `-p - --output-format streaming-json` and writes the prompt to stdin. The headless guide says
  "Headless mode does not read piped stdin into the prompt" and documents `-p <PROMPT>`, `--prompt-file <PATH>` and
  `--prompt-json` only. If `-p -` does not read stdin on the real CLI, switch to `--prompt-file` in a temp file. ◐, confirm first.
- **Grok API-key variable.** The auth guide names `XAI_API_KEY`; secondary sources name `GROK_CODE_XAI_API_KEY`. Pass both.
- **Codex `item/updated`.** The exec parser already handles `item.updated`; no change needed.
- **Grok `streaming-messages-json`.** A fourth output format exists: Claude-shaped `stream-json` (`system/init`, `assistant`,
  `user`, `result`), with `--include-partial-messages` adding `stream_event` deltas. If the plain `text` deltas turn out to be
  coarse, the existing Claude parser could read it. Not verified; not needed for S0.
- **Parsers vs protocols.** The Codex exec and Grok streaming parsers read `type`-tagged lines. App-server and ACP lines carry
  `method` / `id` instead, so today they are all counted as malformed (pinned in `tests/harness_fixtures.rs`). P1 needs a
  JSON-RPC parser per protocol for the live sessions.

## 2. Claude Code cells (re-checked)

| Capability | State | Evidence |
|---|---|---|
| One-shot headless | ✓ | `claude --help`: `-p/--print`, `--output-format stream-json`, `--verbose`. |
| Resume | ✓ | `--resume [value]`, `--fork-session`, `--session-id <uuid>`, `--continue`. |
| Text deltas | ✓ | `--include-partial-messages` run for real; recording kept as `claude-2.1.316-real-deltas.jsonl` (`stream_event` / `content_block_delta` / `text_delta`). |
| Long-lived session | ✓ (flags only) | `--input-format stream-json` ("realtime streaming input"), `--replay-user-messages` (acknowledge stdin messages on stdout). The stdin message format was not exercised here; the mock uses `{"type":"user","message":{"role":"user","content":…}}` ([headless docs](https://code.claude.com/docs/en/headless)). |
| Inject MCP servers | ✓ | `--mcp-config <configs...>`, `--strict-mcp-config`; `--bare` ignores every other MCP source. `system/init` carries `mcp_servers` and `mcp_server_errors` for a config that failed to load. |
| Read-only access | ✓ | `--allowedTools`, `--disallowedTools`, `--tools ""`, `--permission-mode dontAsk`, `--permission-prompts none` (anything that would prompt is denied), `--restricted` (removes Bash and other code runners, confines file tools). Denials show as `permission_denied` system messages and in the result's `permission_denials`. |
| System prompt add-on | ✓ | `--append-system-prompt`; also `--system-prompt`, and the `-file` variants per the docs. Note `--system-prompt-snapshot on` (default): the prompt is recorded on the first request and reused on resume. |
| Cost | ✓ | `total_cost_usd` and `modelUsage` on `result` (recorded). `costBasis:"list"`: an estimate, not the plan bill. |
| Login status | ✓ | `claude auth status` (`--json` default, `--text`). Run here: prints `Login method: …` lines. |
| Built-in model fallback | ✓ | `--fallback-model <model>` (comma-separated list, retries the primary each turn). |

## 3. Rate-limit and auth-expired shapes

All fixtures named `…-mock-rate-limit.jsonl` / `…-mock-auth-expired.jsonl`. Wording marked "mock" is a guess; **the field names are
what matters for classification, and the text is a fallback for regex matching.**

### Claude Code

- **Rate limit.** ✓ real shape for the healthy case: a `rate_limit_event` line with `rate_limit_info {status:"allowed", resetsAt
  (epoch seconds), rateLimitType:"five_hour", overageStatus, isUsingOverage, unifiedWindows:{five_hour:{utilization,resetsAt,status},
  seven_day:{…}}}` (recorded in `claude-2.1.316-real-deltas.jsonl`). ◐ the blocked case, from the docs and strings in the binary:
  `rate_limit_info.status` becomes `"rejected"` (`"allowed_warning"` is the soft warning); a `system/api_retry` line with
  `error:"rate_limit"`, `error_status:429`; an `assistant` line carrying `error:"rate_limit"`; a `result` with `is_error:true`,
  `api_error_status:429`. **Reset time:** `rate_limit_info.resetsAt`, structured. Classify on `rate_limit_event.status=="rejected"` or
  `error=="rate_limit"`. [headless docs: api_retry](https://code.claude.com/docs/en/headless).
- **Auth expired.** ◐ `system/api_retry` with `error:"authentication_failed"` (other categories: `oauth_org_not_allowed`,
  `account_on_hold`, `billing_error`), an `assistant` line with `error:"authentication_failed"`, then `result` `is_error:true`,
  `api_error_status:401`. The binary contains the strings `API Error: 401 Invalid API key · Please run /login` and `Not logged in ·
  Please run /login`. The docs say a failure such as missing authentication is "printed as the result on stdout". Detection:
  `claude auth status`. A `billing_error` (spend limit) is a third kind worth a distinct reason.

### Codex

- **Rate limit (`exec --json`).** ◐ an `error` line then `turn.failed`, both with only `message`: "You've hit your usage limit. … try
  again at Nov 4th, 2026 2:35 AM." The failure comes before `thread.started`, so there is no thread id to resume. The reset time exists
  only inside the prose (formats vary; "try again later" when unknown). No structured code in `exec`. Sources: issues
  [openai/codex#12299](https://github.com/openai/codex/issues/12299), [t3code#27773](https://github.com/pingdotgg/t3code/issues/27773).
  Classify by regex on `usage limit` / `rate limit`.
- **Rate limit (app-server).** ◐ structured: an `error` notification `{error:{message, codexErrorInfo, additionalDetails?}}` then
  `turn/completed` with `status:"failed"`. `codexErrorInfo` is `UsageLimitExceeded` or `RateLimitExceeded` (also `ServerOverloaded`,
  `ContextWindowExceeded`, …; HTTP variants carry `httpStatusCode`). Reset time: `account/rateLimits/read` and the
  `account/rateLimits/updated` notification carry `rateLimits {limitId, primary/secondary {usedPercent, windowDurationMins,
  resetsAt (epoch s)}, rateLimitReachedType}` ([app-server docs](https://developers.openai.com/codex/app-server),
  [protocol.rs](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/protocol.rs)).
- **Auth expired.** ◐ `exec`: `error` lines then `turn.failed`; wording unknown (fixture text is a mock containing `401
  Unauthorized`). App-server: `codexErrorInfo:"Unauthorized"`. Detection: `codex login status` (the docs do not describe its output
  or exit code). The auth guide says Codex refreshes `auth.json` during a run when it can.

### Grok

- **Rate limit.** ✗ not documented anywhere found. The only documented failure shape is `{"type":"error","message":"…"}` then a
  non-zero exit (exit codes: 0 ok, 1 "authentication failure, network error, or runtime error", 130 SIGINT, 143 SIGTERM). Spend fields
  may ride on the `error` line. **Fallback:** classify by regex on the message (`429`, `rate limit`, `quota`, `too many requests`) and
  treat an unclassified exit 1 as a crash for the 3-strike rule in 4.6. No reset time is available. ACP: a JSON-RPC `error.message`.
- **Auth expired.** ◐ exit code 1; the auth guide says Grok refreshes tokens, retries once on 401, and mid-session "the turn fails
  with a re-auth prompt"; "Authentication failed" is the troubleshooting string, the docs' own client sample throws "Run `grok login`
  first, or set XAI_API_KEY". No separate JSON shape. There is no status command (`grok login` / `logout` / `inspect --json` only);
  detection stays file-based (`~/.grok/auth.json`, mode 0600) plus `XAI_API_KEY`
  ([grok-guide/02-authentication.md](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md)).

## 4. Published terms on headless use of a subscription login

Not legal advice; a summary of what each provider publishes, for the risk row in section 6 of the plan. The plan's stance is the
same everywhere: local only, the user's own unmodified CLI and login, no credential read, stored or forwarded by Ratchet.

- **Anthropic** ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)). OAuth login "is intended exclusively
  for purchasers of Claude Free, Pro, Max, Ultra, Team, and Enterprise subscription plans and is designed to support ordinary use of
  Claude Code and other native Anthropic applications." Developers building products "including those using the Agent SDK, should use
  API key authentication"; third parties may not "route requests through Free, Pro, Max, or Ultra plan credentials on behalf of their
  users" nor "collect, store, or intermediate Claude.ai credentials". The same page says this does not prevent "an end user from
  signing in to the unmodified Claude Code binary with their own Claude subscription", and that advertised plan limits "assume
  ordinary, individual usage of Claude Code and the Agent SDK". **Reading:** driving the user's own binary on their own Mac fits;
  the risk is volume. Unattended, racing, round-the-clock optimizations are not "ordinary, individual usage", so expect limit
  stalls and possible enforcement. Mitigations already in the plan: harness fallback, API key option, cloud runs on API keys only.
- **OpenAI** ([auth](https://developers.openai.com/codex/auth), [non-interactive mode](https://developers.openai.com/codex/noninteractive)).
  `codex exec` reuses the saved login by default. API keys are "the recommended default for automation"; ChatGPT-managed auth in CI
  is a documented advanced path for trusted private runners, with `auth.json` treated "like a password" and not for public or
  open-source repositories. The Terms of Use page returned 403 here, so its clauses on automated access were **not read**.
- **xAI** ([auth guide](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/02-authentication.md),
  [overview](https://docs.x.ai/build/overview)). Subscription browser login and `grok login --device-auth` (for SSH/remote machines) are
  documented; for CI and automation the guide recommends an API key and does not say whether subscription login is allowed
  unattended. xAI's own announcements describe SuperGrok / X Premium+ sign-in inside OpenCode and Kilo Code
  ([x.ai/news/grok-opencode](https://x.ai/news/grok-opencode)). One unofficial GitHub comment argues the consumer terms forbid
  automated use of subscription credentials in custom CLIs; it is not xAI's text. xAI's terms page returned 403 here, so
  **unverified**. Treat Grok subscription use as interactive-first and prefer an API key for unattended runs.

## 5. Fixtures

In `crates/ratchet-agents/tests/fixtures/`. `mock` = written from docs/source (◐), never emitted by the real CLI. The existing
fixtures are untouched.

| Behaviour | Claude Code 2.1.316 | Codex 0.166.0 | Grok 1.0.70 |
|---|---|---|---|
| Text deltas | `claude-2.1.316-real-deltas.jsonl` (**real**, trimmed: dropped `commands_changed`, `active_goal`, `autocompact_state`, `post_turn_summary` lines, emptied the long lists in `init`, replaced ids and paths) and `…-mock-deltas.jsonl` | `…-mock-deltas.jsonl` (app-server notifications) | `…-mock-deltas.jsonl` |
| Long-lived transcript | `…-mock-session.jsonl` (two turns, ends each at `result`) | `…-mock-session.jsonl` (app-server: initialize, thread/start, two turns; `"$ID"` marks responses) | `…-mock-session.jsonl` (ACP; `"$ID"` marks responses) |
| MCP tool call | `…-mock-mcp-tool.jsonl` | `…-mock-mcp-tool.jsonl` (`exec` shape) | `…-mock-mcp-tool.jsonl` |
| Read-only refusal | `…-mock-read-only-refusal.jsonl` | `…-mock-read-only-refusal.jsonl` | `…-mock-read-only-refusal.jsonl` |
| Rate limit | `…-mock-rate-limit.jsonl` | `…-mock-rate-limit.jsonl` (`exec` shape) | `…-mock-rate-limit.jsonl` |
| Auth expired | `…-mock-auth-expired.jsonl` | `…-mock-auth-expired.jsonl` (`exec` shape) | `…-mock-auth-expired.jsonl` |

The fake CLI is `crates/ratchet-agents/tests/fake_cli/main.rs`, built as the `ratchet-fake-cli` binary (`replay`,
`claude-stream-json`, `codex-app-server`, `grok-acp`; usage in its header comment). Tests are in
`crates/ratchet-agents/tests/harness_fixtures.rs`.

## 6. P1 result: what the harness layer does with each cell

Code: `ratchet-core/src/ports.rs` (`SessionSpec`, `Access`, `McpServer`, `Capabilities`, `Harness`, `LiveSession`), `ratchet-agents`
(`harness.rs`, `live.rs`, `classify.rs`, `codex_app_server.rs`, `grok_acp.rs`, and the three adapters). Tests:
`tests/harness.rs` (mapping and parsers), `tests/live_sessions.rs` (live sessions against `tests/fake_cli`).

Deviations from the sketch in 4.1, on purpose:

- `LiveSession::send` and `interrupt` take `&self` (not `&mut self`): an interrupt has to run while `send` waits for the turn.
- `Harness::open` takes a `SharedEventSink` (an `Arc`) because the session keeps emitting after `open` returns. The first
  message is `spec.prompt` when it is not empty.
- `RateLimited` and `AuthExpired` are plain event types (`rate_limited`, `auth_expired`); the reset time is `data.resetsAt`
  (epoch seconds) instead of a payload on the enum, so `AgentEventType` stays `Copy` and the wire stays `{"type": "..."}`.
- `Access` defaults to `Edit { network: true }`, which is what `AgentSpec` callers always got; the adapter's own options still
  apply on top of it. `ReadOnly` overrules them.

| Cell | Claude Code | Codex | Grok |
|---|---|---|---|
| One-shot / resume | unchanged (`start_session`/`resume_session` take a `SessionSpec`; `start`/`resume` map into them) | same | same; prompt now in `--prompt-file` (a temp file) instead of `-p -`, resume unchanged |
| Deltas | live: `stream_event` `text_delta` → `Delta`, `thinking_delta` → `Thinking` | live (app-server): `item/agentMessage/delta` → `Delta`, reasoning deltas → `Thinking`. `exec`: whole messages (reasoning items → `Thinking` only if `emit_thinking`) | live (ACP): `agent_message_chunk` → `Delta`. `-p`: `text` lines → `Delta` only if `emit_deltas` (off for optimizations) |
| Live session | `-p --input-format stream-json --output-format stream-json --include-partial-messages --replay-user-messages`; interrupt = `control_request` | `codex app-server`: `initialize` → `initialized` → `thread/start`\|`thread/resume` → `turn/start`; `turn/interrupt` | `grok agent stdio`: `initialize` → `authenticate` → `session/new`\|`session/load`; `session/prompt`; `session/cancel` |
| MCP | `--mcp-config <temp json> --strict-mcp-config`, tools allowed as `mcp__<name>` | `-c mcp_servers.<name>.command/args/env.*`; **fallback** `CodexOptions::config_overlay`: temp `CODEX_HOME` with generated `config.toml`, `AGENTS.md`, `auth.json` linked | live: `session/new {mcpServers}`; one-shot: temp `GROK_HOME` with generated `config.toml` (`[compat.*] mcps=false`) and `auth.json` linked |
| Read-only | `--permission-mode dontAsk`, `--allowedTools Read,Glob,Grep,mcp__*`, `--disallowedTools Bash,Edit,Write,...` | `sandbox_mode="read-only"` (app-server: `sandbox:"read-only"`, `approvalPolicy:"never"`) | one-shot: `--sandbox read-only --tools ... --disallowed-tools ...`; live: tools only (the sandbox is refused in agent mode); permission requests are answered by the client |
| System prompt | `--append-system-prompt` | `-c developer_instructions=...` (overlay: `AGENTS.md`) | one-shot `--rules`; live `session/new _meta.rules` |
| Rate limit | rejected `rate_limit_event` (reset time), assistant `error`, `api_error_status` | `exec`: text; app-server: `codexErrorInfo` + `account/rateLimits/updated` (reset time) | text (reset time only from "retry after Ns") |
| Auth expired | assistant `error: authentication_failed`, 401 | `exec`: text; app-server: `Unauthorized` | text |
| Model fallback | `ClaudeCodeOptions::fallback_model` → `--fallback-model` | ✗ | ✗ |

Fallbacks in force: `Capabilities::live_session` is false when `CliAgentOptions::disable_live_session` is set, and `open` returns a
`SpawnPerTurnSession` (first message = start, later ones = resume) when the live process cannot start or its handshake fails; a
`Status` event says so. Codex over the app-server does not pass an API key through `account/login`; with only `OPENAI_API_KEY`
the app-server may answer `Unauthorized`, which classifies as `AuthExpired` (confirm in real-world testing, then the fix is
`CodexOptions::config_overlay` or per-turn mode).

Tool names for Grok's read-only lists (`GROK_READ_ONLY_TOOLS`, `GROK_WRITE_TOOLS`) are fixtures plus guesses; they are options
(`GrokOptions::read_only_tools`, `write_tools`) so real-world testing can correct them without a code change.

## P2 notes (catalog, keys, preflight)

- **Catalog.** `ratchet-agents/src/models.rs` reads `data/models.json` (bundled). A file named `models.json` in the daemon's support
  directory replaces it; a missing or broken file falls back to the bundled one with a warning in the log. Ids and aliases are
  per harness. An id the catalog does not know goes to the CLI unchanged, with a `tracing` warning. Claude Code and Grok report
  their own dollars, so only Codex entries carry prices today (they feed `CodexPricing::for_model`, and the Codex adapter uses
  the price of the session's model, else the default optimization model). The Codex and Grok ids and the prices are list values
  from public docs and need confirming against the real CLIs (◐).
- **`GET /v1/harnesses`** adds `models`, `defaultModel` (the default for optimizations; the chat default is the model whose
  `defaultFor` has `chat`) and `billing` (`subscription` for a login, `api-key` for a key, absent when signed out). The simulated
  daemon returns no models.
- **`xaiApiKey`.** A new secret next to `anthropicApiKey` / `openaiApiKey`: `PUT/DELETE /v1/settings/secrets/xaiApiKey`,
  `secrets.xaiApiKey` in settings, `ConnectionTarget::Xai`. The runner receives it as `XAI_API_KEY` (the engine already named it
  in `default_secret_names`; the runner puts every fetched secret except `GITHUB_TOKEN` in the agent environment, so
  `AGENT_ENV_PASSTHROUGH` is unchanged). The Grok adapter sets `XAI_API_KEY` and `GROK_CODE_XAI_API_KEY` from it.
- **Preflight.** `desktop/preflight.rs` checks the harness list against `AgentsWhere` when an optimization is created and again
  when it is started: local needs the CLI installed and signed in (or a key in the environment); cloud needs the vendor key the
  runner would receive. The error is `NotConfigured`: "Needs sign-in: Codex is signed out. Run `codex login` ...".

## P6 notes (optimizations: agents list, fallback, tools, no human gate, keep-awake)

Code: `ratchet-core/src/agents_policy.rs` (the policy), `ratchet-agents/src/fallback.rs` (the rules) and `runner_loop.rs` (the loop),
`ratchet-agents/src/control/local.rs` + `local_agents.rs` (the local chain and tool hooks), `ratchet-engine/src/agents.rs` (policy in
the engine), `ratchet-engine/src/sandbox_auto.rs` (environment fallback), `ratchetd/src/power.rs` + `host.rs` (power assertion),
`ratchetd/src/real/service.rs` + `mcp/tools.rs` (daemon side). Tests: `ratchet-agents/tests/fallback.rs`,
`ratchet-engine/tests/engine/{environment_apply,sandbox_auto}.rs`, `ratchetd/tests/api/mcp.rs`, `ratchetd/src/{power,host}.rs`.

### The agents list

`agents:` is an ordered list of harnesses plus three rules (wait limit 30 min, crash strikes 3, re-check 5 min). Two places set it,
`ratchet.yaml` winning:

- the optimization's settings, `opt:<id>:agents_policy` in the engine's key-value store; without it the list is the optimization's own
  agents in creation order, so older optimizations behave as before;
- an optional section in `ratchet.yaml`, either `agents: [claude-code, codex]` or
  `agents: {order: [...], wait_limit_minutes: 30, crash_strikes: 3, recheck_minutes: 5}`.

API: `GET /v1/optimizations/{id}/agents` → `{harnesses, waitLimitS, crashStrikes, recheckS, source: settings|config, summary}`
(`summary` is "Claude Code → Codex → Grok", the order the Agents row shows); `PUT` takes `{harnesses, waitLimitS?, crashStrikes?,
recheckS?}`, answers 400 for an empty list, `mock` or a zero rule, and 409 while `ratchet.yaml` has the section. CLI:
`ratchet agents show <id>`, `ratchet agents set <id> --order claude-code,codex [--wait-limit-minutes N --crash-strikes N
--recheck-minutes N]`. A new list applies to agents launched from then on; a runner keeps the chain it started with.

Each slot's chain is its own harness first, then the rest of the list in order (`AgentsPolicy::chain_from`). Racing keeps working:
two slots on `claude-code` and `codex` each fall back to the other.

### Fallback (plan 4.6), as the runner loop does it

| What the turn did | What happens | Run log |
|---|---|---|
| `rate_limited`, reset time under 30 min | sleeps until the reset, runs the turn again on the same harness | `harness.event = wait`, `resumesAt` |
| `rate_limited`, longer or no reset time | the harness is set aside (until its reset, else 1 h); the next harness starts at the next turn from the brief and the history (CLI sessions do not cross CLIs); the resume message the old session would have got rides along | `harness.event = switch`, `cause = rate-limited` |
| `auth_expired` | switch now | `switch`, `cause = auth-expired`, **`notify`** |
| 3 crashes or malformed streams in a row | switch (a slot with no other harness keeps the old rule: the agent fails) | `switch`, `cause = crashing` |
| every harness unavailable | the agent pauses; every `recheckSeconds` the chain is looked at again (detection, via each adapter's `preflight` or the injected probe); the first harness that is back carries on | `paused` (`data.state = paused`, **`notify`**), then `resumed` (`data.state = running`) and a `switch` if the harness changed |
| the slot's own harness fails preflight at the start | starts on the first harness of the chain that works | `switch`, `cause = unavailable` |

Every entry is a `status` event, so it is a ledger event and shows in the run log: `data.harness = {event, from, to, cause, reason,
detail}` (`wait` carries `harness`, `resumesAt`; `paused` carries `reason`, `recheckSeconds`). `data.notify` is a sentence for the user;
the engine turns each one into a single `notice` event (warning) while the entry stays in the log. A turn that reports a rate limit or
an expired login and made no change is a failed turn, not an idle one.

### Steering notes

`RunnerMessage::User` is queued when it arrives (`status` "user message received", `data.steering = {event: queued, seq, at}`), and
read when a turn starts: one `status` "steering note read" per note, `data.steering = {event: read, seq, queuedAt, readAt, harness,
turn}`. The note rides in the first prompt too (not only in resumes). A note taken by a turn that then failed is repeated to the next
turn, so it is never lost to a switch; its read entry is not written twice.

### The optimization tool set, locally

Every local agent turn gets `mcp__ratchet__*`: the daemon issues the agent a session token for the optimization set, bound to its
optimization and agent, when its loop starts (`LocalRunnerControlOptions::mcp_for`) and revokes it when the loop ends (`mcp_done`).
`AgentSpec` gained `mcp` so the harness adapters' existing flags (`--mcp-config`, `-c mcp_servers.*`, `session/new`) apply to the
runner loop's turns. On the daemon:

- `submit_candidate {sha, note}` → `RunnerMessage::Submit` to the agent's own loop. The loop still submits the branch head with the
  trailer; the note becomes the candidate's description, and a sha that is not the head is reported in the log.
- `connector_query {connector, query}` → `ConnectorQuery` (the connectors the user signed in). Read-only, 2000-character query,
  20 000-character answer. A signed-out or expired connector answers `409 Needs you: ... this optimization carries on without it`:
  the one thing a person has to fix. The only connector that answers today is GitHub (`repo:<owner>/<name>`); other kinds say they do
  not answer queries yet (no metrics connector exists in this codebase).

### No human gate

- `POST /v1/optimizations` with `environmentDraftId` no longer needs an approved draft. `EnvDrafts::settle` runs when it is
  created: a ready draft whose checks all passed is committed (onto its base ref; create the draft with `baseRef: ratchet/config` to
  keep `main` untouched), a failing one is revised once into a smaller setup, and one that still fails is dropped with a `notice`
  ("The environment was not used: ... runs with the standard setup"). Nothing is approved by a person; the approval endpoints stay
  for callers that use them.
- Starting: if the drafted environment cannot be created, the engine tries the standard sandbox (a `notice` says so, kv
  `opt:<id>:environment_fallback`). Only when that fails too does the run fail: status `failed`, stop reason "could not create the
  environment in <region>, even with a smaller setup: ...", plus a `notice` (error). It no longer stays a draft.
- The Overview status word for a finished run with unshipped wins is "Ready to ship" (was "Waiting on you"); `needsYou` keeps only what
  a person must do: open a pull request, look at a failed run, and the new `kind: connector` entry ("Sign in to X again") for a
  signed-out or expired connector.

### Keep-awake

`run-active.optimizationIds` and `active` now mean "a run whose agents run on this Mac is live" (engine: `RunActive.local_ids`); a
cloud-only run no longer holds the Mac. The daemon's hook takes an `IOPMAssertion` (`PreventUserIdleSystemSleep`, FFI in
`ratchetd/src/power.rs`; `caffeinate -i -w <pid>` only if the OS refuses) exactly while `active`. The held state is published as
`keepingAwake` on `run-active` events and `engine.keepingAwake` on every snapshot, settings and health answer.
