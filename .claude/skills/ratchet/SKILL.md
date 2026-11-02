---
name: ratchet
description: Drive Ratchet from the shell with the `ratchet` command. Use when the user asks about their optimizations (list, show, status, spend, wins, report), wants to start, stop or open a pull request for one, wants to talk to Ratchet's assistant (`ratchet chat`), wants Ratchet as an MCP server (`ratchet mcp`), or names Ratchet.
---

# Ratchet

`ratchet` is a client of the Ratchet daemon (`ratchetd`), the same one the Mac app shows. A change you make shows in the
window at once. If the daemon is down, the command starts it (it opens the Ratchet app in the background) and says so; if that
fails it prints what to do.

## Rules

- Always add `--json` and read the JSON. Exit 0 is ok, 1 is a failure (read `error.message` and `error.hint`), 2 is a usage
  error or a command that needs `--yes`.
- Ids are in `ratchet list --json`. A wrong id exits 1 with a hint.
- `run`, `stop`, `pr` and `env end` spend money or change things. Without `--yes` they only print what they would do and exit
  2. Show that preview to the user and ask before you run it again with `--yes`.
- Say "optimization", not "automation".
- To try things without real data, point the command at a simulation: `ratchetd serve --mode sim` in one terminal, then
  `RATCHET_HOME=<its folder> ratchet status --json`.

## Commands you will use most

| Command | Does |
| --- | --- |
| `ratchet status --json` | The daemon and the optimizations at a glance |
| `ratchet list --json` | All optimizations with ids, status, best result, spend |
| `ratchet show <id> --json` | One optimization: agents, baseline vs best, receipt |
| `ratchet run owner/name --budget 50 --time-limit 2h --yes` | Create an optimization and start it (preview without `--yes`; `--demo` runs a simulation) |
| `ratchet stop <id> --yes` | Stop one. Wins found so far are kept |
| `ratchet pr <id> --yes` | Open the pull request with the verified wins |
| `ratchet report <id>` | The markdown report |
| `ratchet open <page or id>` | Show a page (overview, optimizations, benchmarks, connectors, new) or an optimization in the Mac app |

Others: `candidates`, `history`, `spend`, `activity`, `agent pause|resume|message|log`, `events`, `env list|end`, `cost
reconcile`, `repos`, `inspect`, `overview`, `connectors`, and the offline `validate`, `init`, `baseline`, `templates`.

## Chat and MCP

- `ratchet chat new`, `chat send`, `chat show`, `chat stop`, `chat end` talk to Ratchet's assistant: your own Claude Code, Codex
  or Grok, acting on Ratchet through its tools. `chat end` needs `--yes`.
- `ratchet mcp` serves every command above as an MCP tool on stdio, for clients with no shell. Register it with
  `claude mcp add ratchet -- ratchet mcp`. The tools that need `--yes` take `confirm: true` instead.

Everything else: `ratchet --help`, and `ratchet <command> --help` for its options.
