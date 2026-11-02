# Chat engine (P4)

Code: `crates/ratchetd/src/chat` (service, scripted harness, cards, prompt, queue), `crates/ratchetd/src/routes/chats.rs`,
`crates/ratchetd/src/api/chat.rs` (the wire types), `crates/ratchetd/src/real/chat.rs` (the real engine),
`crates/ratchet-ledger/migrations/0002_chats.sql` + `src/chats.rs`, `crates/ratchet-agents/src/chat_harness.rs`,
`ratchet chat` in `crates/ratchet-cli`. Tests: `ratchetd/tests/api/{chat,real_chat}.rs`, `ratchet-ledger/tests/chats.rs`,
`ratchet-cli/tests/cli/daemon.rs`.

## What it is

A chat is a conversation with the user's own agent CLI (Claude Code, Codex or Grok on this Mac) that acts on Ratchet only
through the Ratchet MCP tools (P3). The daemon holds one live CLI session per open chat, hands it a chat-set token scoped to the
chat's repo or optimization, streams what it writes to the app, and turns what it did (a draft, a steering note) into cards.

```text
app ── POST /v1/chats/{id}/messages ─► ChatService ── LiveSession.send ─► CLI (read-only, in the chat's checkout)
 ▲                                        │  ▲                               │  Ratchet tools via `ratchetd mcp --session <token>`
 │  GET /v1/chats/{id}/stream (SSE)       │  └─ tool hook: draft / create / note ─► cards
 └──────────── delta · message · tool · card · note · usage · done · error ◄───┘
```

## API

| Route | Does |
|---|---|
| `POST /v1/chats {repo?, optimization?, harness?, model?}` | 201 `ChatVm`. Harness: request → user default (`chat.defaultHarness` in `engine_kv`) → first signed in (Claude Code, Codex, Grok). 409 when none is ready. `dataNotice` is set while this is the first chat with its harness. |
| `POST /v1/chats/{id}/messages {text}` | 202 `{seq, turn}`; the reply is on the stream. 409 while replying. 1 to 8000 characters. |
| `GET /v1/chats/{id}/stream` | SSE of `ChatStreamEvent`; `Last-Event-ID` resumes. |
| `GET /v1/chats/{id}?after=&limit=` | The chat with its messages (oldest first, `hasMore`), the partial reply (`pending`) and `lastEventId`. |
| `POST /v1/chats/{id}/interrupt` | 204. Stops the reply; what was written is kept. No-op when idle. |
| `DELETE /v1/chats/{id}` | 204. Stops the reply, closes the session (token revoked), removes the checkout, deletes the history. |

Stream events (`event:` = `type`, `id:` counts up per chat; `ready` has no id):

| Event | Carries |
|---|---|
| `ready` | first on every connection: `state`, `lastSeq`, `lastEventId`, `resync` (events since `Last-Event-ID` are gone: refetch), `pending` (the partial reply, when no replay carries it) |
| `message` | a whole message: the user's (echoed) or a finished reply with `harness` and `model` ("Written by Claude Code · Opus") |
| `delta` / `thinking` | partial text / reasoning summary; append. The finished `message` repeats the whole text. Neither is stored. |
| `tool` | a step: `{id, name, state: running|done|failed, summary}`; the first step of a repo chat is `checkout` ("Reading acme/shop") |
| `card` | a draft card `{id, status, rows[], subtitle, error?}`; `replaces` names the draft it supersedes; the same `card.id` again is an update (`created` with the `optimization`, `failed` with an `error`, `replaced`) |
| `note` | a steering note `{status: queued | read, readBy?, readAt?}` |
| `usage` | tokens (incremental) with `billing`; `costUsd` is 0 on a subscription |
| `done` | the turn is over: `outcome` completed / interrupted / failed, turn totals |
| `error` | the turn failed, then `done` follows. `code`: `rate_limited` (with `retry.resetsAt`), `auth_expired`, `unavailable`, `failed`. `title` and `message` are written for people; `fixes` lists the buttons (`retry`, `sign-in`, `switch-harness` + `switchTo`). |

A connection that falls more than 512 events behind is closed; reconnect with `Last-Event-ID`.

### Cards

`draft_optimization` returns the card the s8 screen shows: rows `goal`, `measure`, `agents`, `limits`, `schedule`, `output`,
`config`, each `{key, label, value (as shown), editable, raw (as data), options[]}`. A change is a chat message ("use Grok too");
the chat drafts again and the card updates in place (the old draft becomes `replaced`). Creating from the card and saying "create
it" are the same tool call (`create_optimization`); no step waits on a human.

### Steering

A chat made with `optimization` is bound to it (and to its repo): its token only touches that optimization, the role prompt tells
it to send the user's words with `add_steering_note`, and each note becomes a `note` card, `queued`. A note turns `read` when an
agent of that optimization next does something (a tool step, a commit, a verdict, a message that is not the note itself); P6 can
replace this with a precise signal from the agent's `steering_notes` call.

## Lifecycle

* **Open**: the first message of a chat (or the first after a close) checks the harness is ready, makes the checkout, issues an MCP
  token (`McpHub::sessions.issue(ToolSet::Chat, Scope{repo, optimization})`) and opens a read-only `LiveSession`
  (`Harness::open`, falling back to spawn-per-turn inside the adapters). `resume` is the chat's CLI session id.
* **Checkout**: a repo chat gets a `git worktree` of Ratchet's own clone (`<state>/chats/clones/owner__name`), on a branch
  `ratchet-chat/<id>` from the default branch, at `<state>/chats/worktrees/<id>`. A Home chat gets an empty directory. Never the
  user's working copy. Read-only is the CLI's own sandbox plus no shell tool (`Access::ReadOnly`).
* **Idle**: 10 minutes without a message closes the session and revokes its token; the next message reopens with `resume`. At
  most 4 sessions are held; opening one more closes the one idle the longest.
* **Failure**: a failed turn closes the session (the next message opens a fresh one with `resume`).
* **Restart**: chats and messages are in the ledger; a chat is read back on first use and resumes its CLI session.

## Hot path

Deltas go sink → queue → pump → broadcast. The sink is synchronous and never blocks; neighbouring deltas merge while the pump is
busy and are dropped past 256 KB (the finished `message` has the whole text). No delta touches the ledger or is fetched again; the
partial reply sits in a 64 KB buffer for a client that joins late. Each chat keeps a ring of 256 events for `Last-Event-ID`.
Messages, tool steps, usage, errors and the end of the turn are never dropped.

## Simulation: the scripted harness

`ScriptedChatEngine` is a harness with no model, used by `ratchetd serve --mode sim`, snapshots and tests. It opens like a real one
(read-only spec, the Ratchet MCP server with its token, role prompt, `resume`), calls Ratchet's tools through the daemon with that
token, and remembers its conversation per CLI session id so a reopened session carries on.

| Message | Script |
|---|---|
| names a repo, or the chat has one | looks at the templates, drafts (`$50`, "every night", "weekdays", "use Codex too" are understood) |
| asks for a change | drafts again; the card updates |
| "create it", "go ahead", "yes" | creates the draft |
| any message in a chat bound to an optimization | sends it as a steering note |
| `[simulate:rate-limit]` | `rate_limited`, resets in an hour |
| `[simulate:auth-expired]` | `auth_expired` |
| `[simulate:crash]` | `failed` |
| `[simulate:slow]` | streams until interrupted |

In simulation the daemon drafts into an in-memory git host (`MockHosts`), so a chat creates an optimization with nothing
configured. `SimOptions::chat_chunk_delay` gives the typing effect (15 ms in `serve --mode sim`, none in tests).

## Security

* Read-only access, no shell; the Ratchet tools are the only write path and the daemon checks the tool set and scope on every call.
* The token is random per session, bound to the chat tool set and the chat's repo or optimization, revoked on close, idle close and
  delete, and never stored (not in the ledger, not in a prompt, not in an event).
* The role prompt says: act only through Ratchet tools, propose drafts, create when asked, never edit files, never print a secret.
* The first-use notice says what leaves the Mac: messages, the repo name and the files the CLI reads go to the harness the user
  signed in to; Ratchet sends nothing else.
* Errors shown to the UI are the first line of what the CLI reported, never its stderr.

## CLI

`ratchet chat new [--repo] [--optimization] [--harness] [--model]`, `chat send <id> <text>` (streams the reply; `--json` prints each
event; exits 1 on a failed turn), `chat show <id>`, `chat stop <id>`, `chat end <id>`. They are in the owner-mode MCP tools too
(`chat end` is destructive: it previews until `confirm: true`).
