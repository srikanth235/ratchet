# Agent harnesses — plan of action

Claude Code, Codex and Grok power everything that thinks in Ratchet: optimizations (unattended) and chat (Home composer, creation
conversation, steering). One harness layer under both. Ratchet never ships its own model; it drives the user's CLIs.

## 1. Where we are

Verified in code on `claude/peaceful-lamport-leak6p`.

| Piece | State | Where |
|---|---|---|
| Adapter trait (`start`, `resume`, `preflight`) | done | `ratchet-core/src/ports.rs` `AgentAdapter` |
| Claude Code adapter | done: `claude -p --output-format stream-json --verbose`, resume, permission mode, allowed tools, model | `ratchet-agents/src/claude_code.rs` |
| Codex adapter | done: `codex exec --json`, `exec resume`, sandbox via `-c sandbox_mode`, token-priced cost | `codex.rs` |
| Grok adapter | done: `grok -p - --output-format streaming-json`, resume, permission mode, sandbox, dollar cost | `grok.rs` |
| Shared event model | done: `AgentEvent` Message / Tool / Usage / Status / Error | `ports.rs` |
| Detection (install, version, login, PATH from login shell) | done, all three | `detect.rs`, `GET /v1/harnesses` |
| Local runs in git worktrees | done | `local_agents.rs`, `workdir.rs`, `control/local.rs` |
| Cloud runs (EC2 runner, SSM, S3 inbox) | done | `runner_main.rs`, `runner_loop.rs`, `control/ssm.rs` |
| Where agents run per optimization | enum exists: `AgentsWhere { Local, Cloud }` | `ratchet-core/src/enums.rs` |
| Several agents on one shared ratchet | done (runner loop rebases on `Ratchet` messages) | `runner_loop.rs` |
| Multi-turn agent conversation | done for environment drafts only (start → revise via resume) | `ratchet-env/src/drafter.rs` |
| Stream fixtures | Claude, Codex, Grok (success, resume, failure, noisy) | `ratchet-agents/tests/fixtures/` |
| Cloud API keys | Anthropic, OpenAI, GitHub. **No xAI** | `ratchetd/src/real/settings.rs` |

## 2. Gaps

1. **No chat engine.** Home composer, creation conversation (s8), steering on an optimization: no daemon side, no API.
2. **Agents can't act on Ratchet.** They only see the repo. Chat needs tools: create optimization, run now, read results, add a steering
   note. Optimization agents need connectors (metrics, traces) and steering notes.
3. **No live text.** Claude and Codex text arrives per message; chat would stall for seconds at a time.
4. **No model catalog.** `--model` is passed through, but there's no per-harness list, default, alias or price.
5. **No harness fallback.** A rate limit or an expired login fails the agent; unattended runs stop.
6. **Subscription vs key not modelled.** Cost assumes API pricing; a run on the user's Claude Max login costs no extra dollars but
   counts against plan limits.
7. **Grok can't run in the cloud.** No `XAI_API_KEY` secret.
8. **CLI drift.** Fixtures are hand-captured; no record of which CLI version produced them, no canary against new releases.

## 3. Principles

- **One layer, two uses.** Chat and optimizations call the same `Harness`. A new CLI is one file plus fixtures.
- **Never wait on a human mid-run.** Access is fixed when the session starts. No permission prompts, ever. Anything the agent may not do
  is refused by the tool, not asked about.
- **The user's own login on their Mac; API keys in the cloud.** Subscription credentials never leave the machine.
- **Ratchet's actions are MCP tools.** The same Ratchet MCP server is handed to every CLI. Chat = harness + Ratchet tools.
- **The engine stays the judge.** Agents propose; the trusted bench and protected set decide. Nothing in this plan lets an agent
  touch the evaluator, the protected ref or merge.
- **Tolerant parsers, pinned fixtures.** Unknown events are counted, never fatal. Every fixture names its CLI version.

## 4. Design

### 4.1 Harness layer (`ratchet-agents`)

Extend `AgentAdapter` (keep it source compatible; new methods have defaults) into `Harness`:

```rust
trait Harness: AgentAdapter {
    fn capabilities(&self) -> Capabilities;          // streaming, live_session, mcp, cost_in_usd, ...
    fn models(&self) -> &[ModelInfo];                // catalog, see 4.4
    async fn open(&self, spec: &SessionSpec, events: &EventSink) -> PortResult<Box<dyn LiveSession>>; // chat
}

struct SessionSpec {               // replaces AgentSpec as the input; AgentSpec maps into it
    workdir, prompt, model, env, max_turns,
    access: Access,                // ReadOnly | Edit { network: bool }
    mcp: Vec<McpServer>,           // Ratchet server + connector servers
    system_append: Option<String>, // Ratchet's role prompt
    resume: Option<String>,        // CLI session id
}

trait LiveSession {                // one CLI process held open; chat only
    async fn send(&mut self, text: &str) -> PortResult<()>;   // returns at turn end
    async fn interrupt(&mut self) -> PortResult<()>;
    fn session_id(&self) -> Option<&str>;
    async fn close(self: Box<Self>);
}
```

New event types (additive to `AgentEventType`):

| Type | Meaning | Used by |
|---|---|---|
| `Delta` | partial assistant text | chat UI typing effect |
| `Thinking` | reasoning summary (optional, collapsed in UI) | chat |
| `RateLimited { resets_at? }` | classified from each CLI's error shape | fallback (4.6) |
| `AuthExpired` | login or key rejected | fallback, Agents settings |

**Per-CLI mapping.** ✓ = verified against the CLI here (`claude 2.1.316`) or already in code. ◐ = from docs and source, with a mock
fixture; confirmed in real-world testing. ✗ = missing, fallback below. S0 resolved every former `?`; flags, protocols and source links
are in [harness-matrix.md](harness-matrix.md) (section 1 for these rows, section 3 for rate-limit and auth-expired shapes, section 4
for provider terms).

| Capability | Claude Code | Codex | Grok |
|---|---|---|---|
| One-shot headless | ✓ `-p --output-format stream-json` | ✓ `exec --json` | ✓ `-p - --output-format streaming-json` |
| Resume | ✓ `--resume <id>` (`--fork-session` to branch) | ✓ `exec resume <id>` | ✓ `--resume <id>` |
| Text deltas | ✓ `--include-partial-messages` | ◐ app-server `item/agentMessage/delta` only; `exec --json` sends whole messages (✗ there) ([matrix §1](harness-matrix.md#1-the--cells-of-section-41)) | ✓ `text` deltas (already parsed) |
| Long-lived session | ✓ `--input-format stream-json` on stdin | ◐ `codex app-server` (JSONL JSON-RPC over stdio) ([matrix §1](harness-matrix.md#1-the--cells-of-section-41)) | ◐ `grok agent stdio` (ACP, JSON-RPC 2.0) ([matrix §1](harness-matrix.md#1-the--cells-of-section-41)) |
| Inject MCP servers | ✓ `--mcp-config` + `--strict-mcp-config` | ◐ `-c mcp_servers.<name>.command=…` (inferred from `-c` + config schema) | ◐ ACP `session/new {mcpServers}`; no flag on one-shot `grok -p`, use a per-session `.grok/config.toml` ([matrix §1](harness-matrix.md#1-the--cells-of-section-41)) |
| Read-only access | ✓ `--allowedTools` without Edit/Write/Bash + `--disallowedTools` | ✓ `sandbox_mode="read-only"` | ◐ `--sandbox read-only` + `--tools` / `--disallowed-tools`; in ACP mode the sandbox is refused, so tools only ([matrix §1](harness-matrix.md#1-the--cells-of-section-41)) |
| System prompt add-on | ✓ `--append-system-prompt` | ◐ `-c developer_instructions=…`, else `AGENTS.md` in a temp overlay (`model_instructions_file` replaces, not appends) | ◐ `--rules <TEXT>` (alias `--append-system-prompt`) |
| Cost | USD in `result` | tokens → price table | USD in `end` |
| Login status | ✓ `claude auth status` | ✓ `codex login status` | none → files + `XAI_API_KEY` |
| Built-in model fallback | ✓ `--fallback-model` | ✗ none documented; Ratchet's harness fallback (4.6) | ✗ none documented; Ratchet's harness fallback (4.6) |

Fallbacks when a ◐ cell turns out wrong or a ✗ cell applies: no live session → per-turn spawn + resume (works today, slower first token);
no deltas → whole messages; no MCP flag → write a per-session config into a temp `HOME`/`CODEX_HOME` overlay.

### 4.2 Ratchet MCP server

`ratchetd mcp --session <token>` (stdio). The CLI launches it as a child; it talks to `ratchetd` over the local socket with a
per-session capability token. In the cloud the runner serves the optimization tool set itself (`ratchet-runner mcp`) against its
inbox and artifact store, so no laptop is needed.

Two tool sets, chosen by the token. A session never sees tools outside its set.

**Chat tools**

| Tool | Does |
|---|---|
| `list_optimizations`, `get_optimization`, `get_run`, `get_overview` | read state, results, spend, return |
| `list_templates`, `list_connectors`, `list_repos` | read catalog |
| `draft_optimization {repo, goal, template?, metric?, schedule?, budget?, agents?}` | builds a draft and returns a card id; the config goes to the `ratchet/config` branch, never `main` |
| `create_optimization {draft_id}` | creates it from the draft (the user's own chat message is the go-ahead) |
| `run_now {optimization}`, `pause`, `resume` | control |
| `add_steering_note {optimization, text}` | read by the agents at their next turn |
| `open_pr {optimization, candidate}` | opens the PR for a verified win |
| `explain_result {run}` | bench numbers, guardrails, diffs for the agent to explain |

**Optimization tools**

| Tool | Does |
|---|---|
| `steering_notes` | the user's notes since the last turn |
| `history` | past candidates, verdicts, current best (already built by `context.rs`; exposed on demand) |
| `connector_query {connector, query}` | read-only metrics, logs, traces from the user's connectors (Datadog, Grafana, ...) |
| `submit_candidate {sha, note}` | same as today's commit trailer, explicit |

No tool writes the evaluator, the protected set, budgets or other optimizations.

### 4.3 Chat engine (`ratchetd`)

- New ledger tables: `chats (id, repo?, optimization?, harness, model, cli_session_id, created_at, status)` and
  `chat_messages (chat_id, seq, role, text, card?, usage, at)`.
- Each chat owns a read-only worktree of its repo's default branch (or none, for Home chats without a repo).
- Process: one `LiveSession` per open chat, closed after 10 min idle; the next message reopens it with `resume`.
- API:
  - `POST /v1/chats {repo?, optimization?, harness?, model?}` → chat
  - `POST /v1/chats/{id}/messages {text}` → 202
  - `GET /v1/chats/{id}/stream` (SSE: `delta`, `message`, `tool`, `card`, `usage`, `done`, `error`)
  - `GET /v1/chats/{id}` (history), `POST /v1/chats/{id}/interrupt`, `DELETE /v1/chats/{id}`
- Role prompt (`--append-system-prompt`): you are Ratchet's assistant; act only through Ratchet tools; propose optimizations as drafts;
  never edit files.
- Harness choice: the composer's picker → else the user's default harness → else the first signed-in one from detection.
- Cards: `draft_optimization` returns a card the app renders (the s8 summary card with Create / Edit). Creating from the card or saying
  "create it" in chat are the same call.
- Steering: a chat with `optimization` set gets that optimization's context and the `add_steering_note` tool up front.

### 4.4 Model catalog

`ratchet-agents/src/models.rs`: static table per harness, overridable by a JSON file shipped with the app so it can change without a
release of the engine.

```
ModelInfo { harness, id, label, aliases, default_for: [chat | optimization], price: Option<TokenPrice>, context_window }
```

- Claude Code: aliases (`opus`, `sonnet`, `haiku`) plus full ids; default Opus for optimizations, Sonnet for chat.
- Codex: the OpenAI ids `codex` accepts; prices feed `CodexPricing` (replaces the hard-coded table).
- Grok: the xAI ids `grok` accepts.
- `GET /v1/harnesses` gains `models` and `defaultModel`. Unknown model ids are passed through with a warning, not refused.

### 4.5 Auth and where agents run

| | This Mac (`AgentsWhere::Local`) | Cloud (`AgentsWhere::Cloud`) |
|---|---|---|
| Claude Code | user's login (Pro/Max) or `ANTHROPIC_API_KEY` | `ANTHROPIC_API_KEY` from Secrets Manager |
| Codex | user's ChatGPT login or `OPENAI_API_KEY` | `OPENAI_API_KEY` |
| Grok | user's login or `XAI_API_KEY` | `XAI_API_KEY` (**new secret**) |
| Chat | always this Mac, user's login | — |

- Add `SecretName::XaiApiKey` end to end: settings, `live_config.rs` source map, runner secret refs, Settings UI field.
- Preflight before an optimization starts: every harness on its list must be signed in for where it runs; otherwise the optimization
  shows "needs sign-in" and doesn't start (it never starts and then stalls).
- Local optimizations need the Mac awake: the daemon takes a power assertion (`IOPMAssertion`) while local agents run, and shows it in
  the menu bar.

### 4.6 Unattended reliability

Per optimization: an ordered harness list (`agents: [claude-code, codex, grok]`) and a policy.

1. Error classified as `RateLimited`: wait until `resets_at` if the CLI gives one and it's under 30 min; else switch to the next harness
   on the list at the next turn, carrying the brief and history (not the CLI session; sessions don't cross CLIs).
2. `AuthExpired`: switch immediately; notify once ("Codex sign-in expired; continuing with Claude Code").
3. Crash or malformed stream 3× in a row on one harness: switch.
4. All harnesses unavailable: pause the optimization with the reason; resume automatically when one becomes available (detection
   re-runs every 5 min while paused).
5. Every switch is a ledger event shown in the run log.

Racing (several harnesses on one optimization at once) keeps working as today; the list then means "run these", fallback applies to
each slot.

### 4.7 Cost

- Each `Usage` event carries `billing: Api | Subscription`. Detection already knows `signed-in` vs `api-key`.
- `Api`: dollars as today (CLI-reported, else catalog price × tokens).
- `Subscription`: dollars = 0, tokens recorded; Overview shows "Agents: included in your plan" plus a token count; return on spend
  then counts only AWS spend. Budget caps count only real dollars.
- Forecast (Overview "Next 30 days") uses the same split.

### 4.8 Mac app

- **Home composer**: harness + model picker (chip, like Claude Code's), sends to `POST /v1/chats`, streams via SSE.
- **Creation conversation (s8)**: chat view with the draft card; Create calls `create_optimization`.
- **Optimization → Steer**: chat bound to the optimization.
- **Customize → Agents (s7)**: one row per harness: installed, version, signed in / key / signed out, default model, "use for chat",
  order for optimizations; xAI key field.
- **Optimization settings**: where agents run (this Mac / cloud), harness order.
- `RatchetKit`: `ChatClient` (SSE over URLSession), `ChatStore`; mocks for snapshots.

### 4.9 Security

- Chat sessions: read-only access, no shell (Claude: no `Bash`; Codex: `read-only` sandbox; Grok: read-only profile).
- MCP token: random per session, bound to its tool set and, for chats, its repo or optimization; revoked on close.
- No secret enters a prompt, a tool result or a log. Connector tools return data, never credentials.
- `--strict-mcp-config` (or the equivalent) so the user's own global MCP servers aren't loaded into Ratchet sessions unless they opt in.

### 4.10 Testing

- **Conformance suite** (`ratchet-agents/tests/conformance.rs`): the same scenarios run against every harness through `FakeSpawner`
  and recorded fixtures: start, resume, deltas, tool call/result, usage, rate limit, auth expired, crash, interrupt, MCP tool call.
- **Fixtures carry their CLI version** in the file name (`claude-2.1.316-success.jsonl`); a recorder script
  (`scripts/record-fixtures.sh`) regenerates them from the real CLIs.
- **Live session fakes**: a fake CLI binary (Rust test helper) that speaks each long-lived protocol, for the chat engine tests.
- **MCP server**: tool contract tests (schemas, refusals outside the tool set, token scoping).
- **Chat engine**: SSE ordering, idle close and reopen with resume, interrupt, harness fallback.
- **App**: snapshot tests of composer, chat, card, Agents page in light and dark.
- **Canary (defined, off until real-world testing is approved)**: nightly job that installs the latest CLIs, runs the conformance
  scenarios against them with test keys, and opens an issue on drift.

## 5. Phases

Each phase is one or more Sonnet high-effort sessions. Every phase ends green on `cargo test` and `cargo clippy -D warnings`.

Sessions run on Linux and can't build Swift. For any phase that touches `apps/mac`, the gate is the `mac-app` job in
`.github/workflows/native.yml` (macos-26) on the phase branch: build, unit tests, grid lint, self-test, daemon smoke, light and dark
snapshots, and the layout check. The session pushes, waits for that job, reads its logs, fixes until green, and then looks at the
uploaded snapshots against the mockups.

The review in srikanth235/ratchet#10 is folded in. **F** phases fix what exists; **P** phases build the harness work. The `#10 §n`
references point to that issue's sections. A phase is done only when every #10 item it owns is ticked.

| # | Phase | Depends on | Sessions | #10 items |
|---|---|---|---|---|
| S0 | Spike: confirm the `?` cells | — | 1 | — (done) |
| P1 | Harness layer + new events | S0 | 1 | — |
| F0 | Hotfixes | — | 1 | Fix first: keep-awake `Lagged` bug, errors shown, launch at login; §7 `--yes` on CLI |
| F1 | Mockups v0.3.1 | — | 1 | Fix first: one value story, PR-free config, control borders; §5 mockup items; §6; §3 mockup items; empty/error/light states |
| F2 | App foundation: architecture + performance | F0 | 1–2 | §1, §2 |
| F3 | App foundation: design system, kit, icon | F1 | 1 | §4 |
| F4 | App foundation: native shell + Liquid Glass | F2, F3 | 1 | §3 app items, §6 app items |
| F5 | Test, build, OS, CLI tooling | F0 | 1 | §7 (except MCP and keep-awake, see P3 and P6) |
| P2 | Model catalog, xAI key, auth matrix, preflight | P1 | 1 | — |
| P3 | Ratchet MCP server (session + owner mode) | P1 | 1 | §7 MCP |
| P4 | Chat engine + API | P1, P3 | 1–2 | — |
| P5 | Mac app: composer, chat, steering, Agents page | P4 (API), P2, F4 | 2 | §5 chat, composer, draft card, steering, agents items |
| P6 | Optimizations: harness list, fallback, MCP for agents, no-wait environment | P1, P2, P3 | 1 | Fix first: no "Waiting on you"; §7 local-only keep-awake |
| P7 | Cost split + Overview | P2, P6 | 1 | §5 Overview items |
| P8 | Conformance suite, fixture recorder, canary definition | P1–P6 | 1 | — |

**Parallel tracks.** Rust: P1 → (P2 ∥ P3) → (P4 ∥ P6) → P7 → P8. App: F0 → (F1 ∥ F2 ∥ F5) → F3 → F4 → P5. The tracks meet at P5, which
starts on mocks once the P4 API is fixed. Parallel sessions push to their own branches (`claude/harness-<phase>`). The orchestrator
merges each into `claude/peaceful-lamport-leak6p` and re-runs the checks before dependants start.

### Skills every session loads

The app's scope and features are growing, so every session loads the relevant Bama skills (`.claude/skills/`) before writing code. It
then runs each skill's own checks before committing. The session prompt names the skills, and the session says which it used in its
summary.

| Phase | Skills |
|---|---|
| Any Swift change | `swift-swiftui`, `build-and-verify`, `test-mac-app` |
| Any screen, sheet or control | `mac-design`, `layout-grid`, `ux-design`, `design:accessibility-review` |
| DesignKit, tokens, parts, icon (F3) | `design-kit`, `design-system`, `app-icon` |
| Refactors (F2, P5 start) | `improve-architecture` |
| Hot paths: streaming, lists, charts (F2, P4/P5) | `performance` |
| Menu bar, login item, power, URL scheme, notifications (F0, F5, P6) | `os-integrations` |
| CLI and MCP (F0, F5, P3) | `agent-cli` |
| New flows or screens (F1, P5) | `user-stories` (acceptance criteria), `ux-design` |
| Mockups (F1) | `design:design-critique`, `design:accessibility-review` |
| Phase end, any app phase | `improve-loop` rules for story-by-story verify; `design:design-critique` on the new snapshots |

### F0 — Hotfixes

- ratchetd keep-awake loop: continue on `RecvError::Lagged` and resync from `run_active()` (`crates/ratchetd/src/main.rs:324-330`).
- Show `AppStore.lastError` in a dismissible banner and clear it when the next action starts.
- "Launch at login" via `SMAppService.mainApp`, starting with only the menu bar item, so notifications keep coming.
- CLI `run`, `stop`, `env end` and `pr` require `--yes`; without it they print a preview.
- **Done when** a test proves the loop survives `Lagged`, the banner has a snapshot, and the CLI contract tests cover `--yes`.

### F1 — Mockups v0.3.1

- Apply every mockup item in #10 to `design/v03.html` and the prototype, keeping the full scope.
- Add empty, error, loading, daemon-down and light-mode screens, and render the PNGs again.
- **Done when** the critique and accessibility checks pass again with no 🔴 items left.

### F2 — App foundation: architecture and performance

- #10 §1, in order, one commit per step with no behaviour change:
  - protocol split
  - `SSEConnection`
  - `DaemonRatchetClient` extractions
  - mock split and contract suite
  - store split and `EventReducer`
  - navigation
  - `Harness` naming
- #10 §2: event coalescing, capped refresh, per-section errors, observable split, log race, formatter, lazy lists, off-main cache,
  eviction, structured event task.
- **Done when** contract tests pass against both clients and the self-test passes.

### F3 — App foundation: design system, kit and icon

- `design/KIT.md`, `design/DESIGN-SYSTEM.md`, `KitGallery`, forced states and focus rings.
- Accent `.tint`, tokens reconciled with v0.3.1, `Layout` enum, Increase Contrast and Reduce Motion.
- The v0.3 parts: composer, chip, segmented control, toggle, settings sidebar.
- Icon: legible at 16 pt, a dark variant, one source.
- **Done when** the gallery snapshots show every part in every state in light and dark, and the grid lint is clean on all of Sources.

### F4 — App foundation: native shell and Liquid Glass

- System sidebar, toolbar with New, search and filters, Settings scene at 520 wide, context menus with `Table`, and menu commands.
- No custom backgrounds behind sheets and bars, glass on the floating composer, and chrome that dims when the window is inactive.
- Window title and scene restore.
- **Done when** the snapshots match v0.3.1 and the `mac-design` and `layout-grid` checks pass.

### F5 — Test, build, OS and CLI tooling

- Debug bridge, `ratchet://` URL scheme and `ratchet open`, self-test audits, missing snapshot scenes.
- `build.sh` restarts the daemon, and the app checks the daemon version.
- CLI install to `~/.local/bin`, a `.claude/skills/ratchet` skill for agents, and the CLI auto-starts the daemon.
- **Done when** an agent can drive the running app through the bridge in a test.

### S0 — Spike on mocks (1 session)

No CLI installs. Codex and Grok are not installed in sessions; Claude Code is (`2.1.316`).

- For each `?` cell in 4.1, read the CLI's published docs and open source (Codex is open source; Grok's ACP output is already parsed
  in `grok.rs`) and write down the flag or protocol, with the source link.
- Write **mock fixtures** from that: one per behaviour per CLI (deltas, live session, MCP tool call, read-only refusal, rate limit,
  auth expired). File names say `mock` and the documented CLI version: `codex-0.166.0-mock-rate-limit.jsonl`. Claude ones that can be
  produced without a login are recorded for real.
- A **fake CLI binary** (`ratchet-agents/tests/fake_cli`) that replays a fixture and speaks each long-lived protocol, for P1 and P4.
- **Done when** `docs/harness-matrix.md` gives every cell one of: ✓ verified (real CLI), ◐ from docs (mock fixture), ✗ missing +
  fallback; and the fixtures and fake CLI are committed.
- Every ◐ cell keeps its fallback wired (spawn-per-turn + resume, whole messages, config overlay), so a wrong guess degrades chat
  speed, never correctness.
- ◐ cells are confirmed with the real CLIs during real-world testing (not approved yet); a mismatch is a parser fix plus new
  fixtures.

### P1 — Harness layer

- `SessionSpec`, `Access`, `McpServer`, `Capabilities`, `Harness`, `LiveSession` in `ratchet-core` ports; `AgentSpec` maps into
  `SessionSpec` so existing callers keep working.
- `Delta`, `Thinking`, `RateLimited`, `AuthExpired` events; each parser emits them.
- Each adapter: `capabilities()`, access → flags, MCP → flags/overlay, `--append-system-prompt` equivalent.
- `LiveSession` per CLI (Claude `--input-format stream-json`; Codex/Grok per S0), with fallback to spawn-per-turn.
- **Done when** existing tests pass unchanged and new adapter tests cover every row of the 4.1 table from fixtures.

### P2 — Catalog, keys, preflight

- `models.rs` + bundled `models.json`; `CodexPricing` reads from it.
- `SecretName::XaiApiKey` through settings, live config, runner secrets, OpenAPI, Settings UI.
- `GET /v1/harnesses` adds `models`, `defaultModel`, `billing`.
- Optimization preflight by harness list × where.
- **Done when** a cloud optimization with Grok receives `XAI_API_KEY` on the runner (simulation test), and preflight refuses a
  signed-out harness with a clear reason.

### P3 — Ratchet MCP server

- New crate `ratchet-mcp` (stdio JSON-RPC, MCP spec), tool sets from 4.2, schemas generated from `ratchet-core` types.
- `ratchetd mcp --session <token>`; `ratchet-runner mcp` for the cloud tool set.
- Token issue/revoke in `ratchetd`.
- Owner mode: `ratchet mcp`, which reads `ratchetd.token`, offers one tool per CLI command generated from the CLI's command table,
  and can be registered with `claude mcp add` (#10 §7, `agent-cli`).
- **Done when** each CLI (via fixtures/fake) lists exactly its tool set; out-of-set calls are refused; `draft_optimization` writes to
  `ratchet/config` only (tested against the fake git host).

### P4 — Chat engine

- Ledger migrations, `ChatService`, worktree lifecycle, live-session pool with idle close, SSE route, OpenAPI.
- Role prompt, card events, steering binding.
- Simulation mode: a scripted chat harness so the app works offline and in snapshots.
- **Done when** a scripted chat creates an optimization end to end in simulation; SSE delivers deltas in order; interrupt and
  reopen-with-resume work.

### P5 — Mac app

- `ChatClient` (SSE), `ChatStore`, mocks.
- Home composer with harness/model picker; creation conversation with draft card; Steer on optimization detail.
- Agents page (s7) per 4.8; optimization settings: where + harness order.
- #10 §5 items for these screens:
  - draft card: popovers per row, Undo, Stop / Start Over, errors, "Create and Run Once" with its cost
  - steering timeline with read receipts
  - replies marked with agent and model, Retry / Copy, first-use data notice
  - composer chips as popovers
  - every broken status shows a button to fix it
  - editable optimization settings rows (Agents, Value, Archive)
  - Run header with status, Stop, Pause Schedule and Open PR
- **Done when** build-and-verify snapshots match the v0.3 mockups (s1, s7, s8) in light and dark, and the in-app self-test drives a
  simulated chat to a created optimization.

### P6 — Optimizations

- `agents:` list + policy in optimization settings (ledger) and `ratchet.yaml` (optional override).
- Fallback state machine in the runner loop (4.6) with ledger events; auto-resume when a harness returns.
- Optimization tool set attached to every agent turn; steering notes read at each turn.
- Power assertion (`IOPMAssertion`) only while local agents run, shown in the menu bar (#10 §7).
- No human gate: drop the "Waiting on you: approve the environment" state; checks run on their own and fall back to a smaller setup;
  "Needs you" only for what a person must do (#10 Fix first).
- **Done when** simulation tests cover: rate limit → wait; rate limit long → switch; auth expired → switch; all down → pause →
  resume; steering note reaches the next turn.

### P7 — Cost

- `billing` on usage; ledger columns; budget guard counts dollars only.
- Overview: spend split (AWS / agents API / included in plan), return computed on dollars; forecast split.
- One value story on Home and Overview ("delivered" + "pending"), a labelled spend line, a "pending" label in the table, an empty
  state, and a Value row the footnote links to (#10).
- **Done when** Overview snapshots show both a key-billed and a subscription optimization correctly.

### P8 — Conformance and canary

- `conformance.rs` over all harnesses; versioned fixture names; `scripts/record-fixtures.sh`.
- Canary workflow file committed **disabled** until real-world testing is approved.
- **Done when** adding a fourth harness only needs an adapter file + fixtures to pass the suite (proved with a test-only fake
  harness).

## 6. Risks

| Risk | Mitigation |
|---|---|
| A CLI changes its stream format | tolerant parsers, versioned fixtures, canary, minimum version in detection |
| Mock fixtures (◐) don't match the real Codex / Grok | each ◐ cell keeps a working fallback; confirmed in real-world testing; canary catches drift |
| Codex or Grok lack a long-lived mode | spawn-per-turn + resume (works today); chat is slower to first token |
| Provider terms on driving a subscription login headlessly | local only, the user's own CLI and login, no credential leaves the Mac; S0 notes each provider's published terms |
| Plan rate limits stall unattended runs | fallback across harnesses; wait-until-reset; pause/auto-resume |
| Mac sleeps during a local optimization | power assertion; recommend cloud for long runs |
| Agent escapes read-only in chat | read-only enforced by each CLI's own sandbox + no shell tool; Ratchet tools are the only write path |
| User's own MCP servers leak into sessions | strict MCP config by default |

## 7. Decisions (defaults used unless changed)

1. Subscription logins: this Mac only; cloud uses API keys. **Default: yes.**
2. Chat uses the user's own login, not a Ratchet-paid model. **Default: yes.**
3. Chat default harness: first signed-in of Claude Code → Codex → Grok; user can change it.
4. Optimization default harness list: all signed-in harnesses, Claude Code first.
5. Creating an optimization from chat needs the user's message or a click on the card; nothing else waits on a human.

## 8. Not in this plan

Release (signing, notarization, update) and real-world testing, both not approved. Other CLIs (Gemini, Cursor, Aider): the harness
layer makes them one file each later.
