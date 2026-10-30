# Real-environment testing: agents (`@ratchet/agents`)

Everything here is written against recorded or fake CLI output. Nothing in `packages/agents` has been run against
a real `claude` or `codex` binary, a real EC2 runner, SSM or S3. This page says what is mocked, what to verify,
and how to turn each mock into a real test. It is the checklist for **spike A** (PLAN.md Phase 0) and the first half
of **M3/M4**.

## What is real and what is mocked

| Piece | Tested against | Real-environment gap |
|---|---|---|
| `ClaudeCodeAdapter` | Hand-written transcripts in `test/fixtures/claude-*.jsonl` shaped like `claude -p --output-format stream-json --verbose` output, through a fake spawner | Real CLI flags, event shapes, resume behaviour, exit codes |
| `CodexAdapter` | `test/fixtures/codex-*.jsonl` shaped like `codex exec --json`, fake spawner | Same, plus the resume subcommand syntax and sandbox config keys |
| Process handling (`nodeSpawner`, `runProcess`) | **Real** child processes: stdin, stderr tail, process-group kill including a grandchild that ignores SIGTERM | none (re-run on the runner AMI's Linux, the container here is also Linux) |
| `runRunnerLoop` | **Real** `git` against a temp repo with a bare remote, `ScriptedAgent`, fake verdicts | Real GitHub remote and token, real latency |
| `ScriptedAgent` + `scripts/demo-search` | Real patches against `examples/demo-search`; each good patch keeps the demo's 65 tests green | none (it is the mock) |
| `LocalRunnerControl` (clone mode) | Real clones, in-process loop | n/a (simulation only) |
| `LocalRunnerControl` (worktree mode, local agents) | Real `git worktree`s against a bare remote; the real `ClaudeCodeAdapter` driving a **shell stand-in** for `claude` (env, cwd, binary path, no API key); two agents sharing one clone | The real `claude` / `codex` on a developer machine: see "Local agents with your own login" below |
| `detectHarnesses` / `resolveLoginPath` | Fake spawner and file system for every branch; shell stand-ins for the CLIs through real processes; `codex login status` wording checked against codex-cli 0.166 on Linux | `claude auth status` (never run: the CLI could not be installed here), the macOS keychain probe, a GUI-launched Electron app's PATH, `codex login status` for a ChatGPT login |
| `SsmRunnerControl` | In-memory `CloudCompute` + `ArtifactStore` | Real SSM `SendCommand` (quoting, 24 KB script limit, timeouts), real S3 |
| `AwsCliArtifactStore` | Fake `aws` binary | Real `aws s3 cp` / `s3api list-objects-v2` behaviour and IAM |
| `ratchet-runner` bin | Spawned as a real subprocess with a filesystem artifact store and the mock agent | Real instance profile, SSM secrets, systemd/`setsid` launch |

## Conventions the engine and the AMI must honour

**Artifacts layout** (all under the per-agent prefix the IAM boundary allows):

```
artifacts/<opt>/<agent>/config.json              RunnerConfig, written by SsmRunnerControl.start
artifacts/<opt>/<agent>/events/00000001.jsonl    runner -> engine, batches of AgentEvent JSON lines
artifacts/<opt>/<agent>/inbox/00000001.json      engine -> runner, one RunnerMessage per object
artifacts/<opt>/<agent>/done.json                final RunnerLoopResult, written on exit
```

**Git.** The runner expects `config.repoUrl` to contain branch `config.branch` (or `config.baseRef` to branch from) and
pushes candidates with `git push --force-with-lease origin HEAD:refs/heads/<branch>`. The ratchet best is read from the
remote branch **`ratchet/best`** (override with `RATCHET_BEST_REF`); the engine must keep it pointing at the current best
commit. This is a convention the `RunnerMessage` verdict does not carry; see "Proposed port changes" below.
Local refs: `refs/ratchet/accepted` is the newest tree known good (the best, once fetched). Agents can
`git reset --hard refs/ratchet/accepted` to drop rejected work.

**`ratchet-runner` environment** (set by `SsmRunnerControl.start` through `runnerEnv`, or by the AMI's unit file):

| Variable | Meaning |
|---|---|
| `RATCHET_OPTIMIZATION_ID`, `RATCHET_AGENT_ID` | required; must match the config |
| `RATCHET_ARTIFACTS_BUCKET` | S3 bucket (uses the `aws` CLI and the instance profile). Alternatively `RATCHET_ARTIFACT_DIR` for a local directory store |
| `RATCHET_SECRET_REFS` | `NAME=/ssm/param,NAME2=/ssm/param2`, fetched with `aws ssm get-parameter --with-decryption`. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` go to the agent CLI; `GITHUB_TOKEN` is used for git only and is never given to the agent |
| `RATCHET_RUNNER_CONFIG` / `RATCHET_RUNNER_CONFIG_FILE` | config inline or from a file instead of `config.json` in the store |
| `RATCHET_WORKDIR` / `RATCHET_WORK_ROOT` | clone location (default `$TMPDIR/ratchet/<opt>/<agent>`) |
| `RATCHET_BEST_REF`, `RATCHET_MODEL`, `RATCHET_MAX_IDLE_TURNS` | loop options |
| `RATCHET_CLAUDE_BIN`, `RATCHET_CODEX_BIN`, `RATCHET_CLAUDE_PERMISSION_MODE` | adapter options |
| `RATCHET_EVENT_FLUSH_MS`, `RATCHET_INBOX_POLL_MS`, `RATCHET_CLOCK_SPEED` | tuning; clock speed > 1 is for simulation only |
| `RATCHET_MOCK_SCRIPT` | for `kind: mock`: `cc`, `cx` or a script JSON path |

The agent CLI gets a deliberately small environment (`PATH`, `HOME`, `USER`, `LANG`, `LC_ALL`, `TERM`, `TMPDIR`,
`SHELL`, `AWS_REGION`, `AWS_DEFAULT_REGION` and the model key). `SIGTERM`/`SIGINT` abort the running turn (process group
killed) and the runner exits 0 with `done.json.reason = "aborted"`. Exit code 1 = `failed`/`agent-unavailable`,
2 = bad configuration.

## Local agents with your own login ("This machine")

The default mode needs no AWS for the agents at all: the app drives the `claude` / `codex` you already use, signed in
the way you already are. Do this first, before the EC2 steps below; it is the fastest way to find out whether the real
flags and event shapes match the fixtures.

Prerequisites: `claude` and/or `codex` installed and signed in (`claude` once interactively, `codex login`), a scratch
repository you can push branches to (a fork of `examples/demo-search`), and a GitHub token that can push to it.

### 0. Detection

```ts
import { detectHarnesses, resolveLoginPath } from '@ratchet/agents';
const login = await resolveLoginPath();            // what $SHELL -lc 'echo $PATH' gives, plus common install dirs
console.log(login);
console.log(await detectHarnesses({ loginPath: login.path }));
```

Verify, per harness: `installed`, `version`, `path` are right; `auth` matches reality in all four states (signed in,
API key only via env, signed out, binary missing). Specifically:

- **From a GUI launch (the part that matters).** Run the same call from the packaged app launched from the Dock /
  Finder, not from a terminal. `fromLoginShell` must be `true`, and `claude` / `codex` must be found even though the
  app's own PATH is the bare launchd one. If your shell sets PATH only in `~/.zshrc` (nvm, volta shims), the login shell
  will not see it; the fallback directories (`~/.nvm/versions/node/*/bin`, `~/.volta/bin`, ...) should still find the
  binary. If not, add the directory to `fallbackPathDirs`.
- **Claude Code auth.** `claude auth status` is assumed to print JSON with `loggedIn` (and `authMethod`) and exit non-zero
  when signed out. **Not verified**: record the real output of both states in `test/fixtures` and fix `claudeAuth` if it
  differs. Check the credential-file fallback (`~/.claude/.credentials.json`, or `$CLAUDE_CONFIG_DIR`) and, on macOS, that
  `security find-generic-password -s "Claude Code-credentials"` (no `-w`) exits 0 with a login and 44 without, and that it
  never raises a keychain prompt.
- **Codex auth.** Verified on codex-cli 0.166: `codex login status` writes to **stderr**; "Not logged in" exits 1,
  "API key configured - sk-***" exits 0. Not verified: the wording for a ChatGPT login ("Logged in using ChatGPT" is
  assumed) and whether `OPENAI_API_KEY` / `CODEX_API_KEY` in the environment change the answer (here they did not).
- **No secrets.** `JSON.stringify` the result and the logs: no key, token or masked key text may appear.
- **Timeouts.** Put a `sleep 30` script first on PATH under the name `codex`: detection must return `installed: false`
  with a "timed out" hint after about 5 s, never hang the app.

### 1. One local turn, by hand

```bash
export PATH="$(resolve the login PATH from step 0)"
git clone <your fork> /tmp/rt-local && cd /tmp/rt-local
env -i HOME="$HOME" PATH="$PATH" claude -p --output-format stream-json --verbose --permission-mode acceptEdits \
  --allowedTools Bash,Read,Edit,Write,Glob,Grep <<< "add a comment to README.md"
```

With no `ANTHROPIC_API_KEY` in that environment the turn must run on your login. Check that the stream has an init line
with a session id, a `result` line with `total_cost_usd` (for a subscription this is a notional figure; confirm it is
present, because the engine's budget rule runs on it), and that `--resume <id>` continues the session. Same for
`codex exec --json -c sandbox_mode="workspace-write" -` with `CODEX_HOME` untouched.

### 2. The local control, no engine

```ts
import { createLocalAgentControl, detectHarnesses, resolveLoginPath } from '@ratchet/agents';
const { path: loginPath } = await resolveLoginPath();
const control = createLocalAgentControl({
  rootDir: '/tmp/ratchet-local', harnesses: await detectHarnesses({ loginPath }), loginPath,
  gitEnv: gitAuthEnv(repoUrl, token),
});
```

Start one agent through `control.start(handle, config)` (use `packages/agents/test/harness.ts` for a config), answer its
`commit` events with `verified` by `control.send`, and check:

- the worktree exists at `/tmp/ratchet-local/<opt>/<agent>` with a `.git` **file**, the clone at `.../clone`;
- the commit is authored by `<agent> (ratchet)` unless your global git identity is set (then yours; both are fine) and is
  not signed (`commit.gpgsign=false` in the worktree), so no pinentry prompt can block an unattended turn;
- the branch reached the remote with the `Ratchet-Candidate: true` trailer;
- the agent process environment has **no** `ANTHROPIC_API_KEY`, `GITHUB_TOKEN`, `AWS_*` (add `env | sort > /tmp/env.txt`
  to a prompt, or run the shim from `test/local-agents.test.ts`), and `PATH` starts with the login PATH;
- `control.stop(handle)` removes the worktree and leaves the branch on the remote; `control.release(optId)` removes the clone;
- with two agents at once, `git -C <clone> worktree list` shows both and their `refs/ratchet/<agent>/accepted` refs differ.

### 3. Through the engine

`createHybridRuntime({ ..., sandbox: null })` with real settings (profile, region, GitHub, bench image) and the fork:
create an optimization with `agentsWhere: 'local'`, start it, and watch the sandbox appear (`sandbox` events:
`creating`, `ready`), the baseline run on the bench instance, the agent commit, the bench / confirm verdict, and the
sandbox disappear after the idle window (set `sandboxIdleMs: 120000` for the test) with `teardownReceipt(id)` showing
`nothingRunning: true`. In the AWS console, after the receipt: no tagged instances, no buckets, no `ratchet-*` roles.

Things this mode cannot prove from here and that the first real run has to answer:

- an unattended `claude -p` with `--allowedTools Bash,...` and `acceptEdits` runs commands on your machine without a prompt;
  decide whether that is acceptable for your repo before pointing it at anything sensitive;
- Claude Code subscription limits: a long run may hit the 5-hour window; the loop sees an `error` result and retries up to
  `maxConsecutiveFailures`, then the agent is `failed`. See "Terms and authentication" below for whether a subscription
  may be used for automated runs;
- macOS: does the Dock-launched app inherit enough environment (`HOME`, `SSH_AUTH_SOCK` is deliberately removed) for the
  CLI to find its keychain item?
- the app being closed or the Mac asleep stops local agents (use `agentsWhere: 'cloud'` for those runs).

## Testing the real adapters on an EC2 runner

Prerequisites: the runner AMI (built in "Build images" below) with Node 22, git, `claude`, `codex`, AWS CLI, profilers;
an agent role with the IAM boundary from `docs/real-env/aws.md`; the model key in SSM
(`/ratchet/<opt>/<agent>/ANTHROPIC_API_KEY`, `.../OPENAI_API_KEY`) and a repo-scoped GitHub token; a test repo (use a
fork of `examples/demo-search`).

### Build images

The runner AMI is defined in `infra/images/runner.pkr.hcl` (scripts in `infra/images/scripts`). It installs Node 22,
pnpm, git, AWS CLI v2, the SSM agent, perf/strace/sysstat, hyperfine, k6, the `claude` and `codex` CLIs at pinned,
checksum-verified versions (`infra/images/scripts/versions-agents.sh`) and `ratchet-runner` from tarballs packed out of
this checkout (`@ratchet/core` + `@ratchet/agents`). No model keys or tokens are baked in: the key and the GitHub token
come from SSM at launch (`RATCHET_SECRET_REFS`).

```sh
export AWS_PROFILE=ratchet-sandbox AWS_REGION=us-east-1
infra/images/build.sh runner        # `both` also builds the bench AMI (see aws.md, "Build images")
# runnerImageRef: ami-0123456789abcdef0
```

Pass the id as `runnerImageRef` to `createLiveRuntime` (`packages/runtime/src/live.ts`; it reaches the engine as
`settings.runnerImageRef` and `RunInstances.ImageId`), next to `benchImageRef` for the bench AMI. On the instance:

| What | Where |
|---|---|
| `ratchet-runner` | `/usr/local/bin/ratchet-runner`: a launcher that first loads `/etc/ratchet/runner.env` (written by the instance user data: secret refs, proxy) without overriding variables the caller already set, then runs the bin installed in `/usr/local/lib/ratchet-runner` |
| `claude`, `codex` | `/usr/local/bin`, version-pinned; Claude Code's auto-updater is disabled in `/etc/claude-code/managed-settings.json` so the pin holds |

When you bump a CLI pin, rebuild and rerun sections 0-2 below: the adapters parse the CLIs' output formats. Remaining gaps are listed at the end of `infra/images/README.md`.

### 0. Smoke the binaries by hand (this is Phase 0 spike A)

```sh
export ANTHROPIC_API_KEY=...            # from SSM on the runner
cd /work/demo && git checkout -b spike
claude --version
# one turn, raw transcript captured for the fixtures
echo 'List the files in this repo and say hello.' | \
  claude -p --output-format stream-json --verbose --permission-mode acceptEdits \
  --allowedTools Bash,Read,Edit,Write,Glob,Grep | tee /tmp/claude-turn1.jsonl
SID=$(jq -r 'select(.type=="system" and .subtype=="init") | .session_id' /tmp/claude-turn1.jsonl)
echo 'Now say goodbye.' | claude -p --output-format stream-json --verbose --resume "$SID" | tee /tmp/claude-turn2.jsonl
```

```sh
export OPENAI_API_KEY=...
echo 'List the files in this repo and say hello.' | codex exec --json --skip-git-repo-check -c sandbox_mode='"workspace-write"' - | tee /tmp/codex-turn1.jsonl
TID=$(jq -r 'select(.type=="thread.started") | .thread_id' /tmp/codex-turn1.jsonl)
echo 'Now say goodbye.' | codex exec resume "$TID" --json --skip-git-repo-check - | tee /tmp/codex-turn2.jsonl
```

Copy the transcripts into `packages/agents/test/fixtures/` as `real-claude-*.jsonl` / `real-codex-*.jsonl` (strip
anything sensitive). `test/real-fixtures.test.ts` parses every `real-*` fixture and fails if the parser loses the session
id, drops a usage event, or skips a known event type, so a CLI release that changes the format shows up as a red test.
Compare `parser.unknownTypes` for each real transcript and decide whether any unknown type matters.

### 1. One adapter turn through the package

```sh
cd packages/agents && pnpm build
node --input-type=module -e '
import { ClaudeCodeAdapter } from "./dist/index.js";
const a = new ClaudeCodeAdapter();
console.log(await a.preflight());
const s = await a.start({ workdir: "/work/demo", prompt: "Say hello in one sentence." }, (e) => console.log(JSON.stringify(e)));
await a.resume(s, { workdir: "/work/demo", prompt: "" }, "Now goodbye.", (e) => console.log(JSON.stringify(e)));
console.log(s);
'
```

Check: events stream while the turn runs (not only at the end), `usage` arrives once per turn with a sane `costUsd`, `s.id`
is stable (or changes on resume, see below), killing the process with Ctrl-C leaves no orphaned `claude`/`node` children
(`pgrep -af claude`). Repeat with `CodexAdapter` and `OPENAI_API_KEY`.

### 2. Full runner loop against a real remote (no AWS control plane yet)

```sh
export RATCHET_OPTIMIZATION_ID=spike RATCHET_AGENT_ID=cc RATCHET_ARTIFACT_DIR=/tmp/artifacts RATCHET_WORKDIR=/work/spike-cc
# config.json: a RunnerConfig with kind "claude-code", repoUrl = your fork, branch = ratchet/cc, brief = RATCHET.md
mkdir -p /tmp/artifacts/artifacts/spike/cc && cp config.json /tmp/artifacts/artifacts/spike/cc/config.json
ratchet-runner
# in another shell, play the engine: watch events, answer a commit with a verdict
tail -f /tmp/artifacts/artifacts/spike/cc/events/*.jsonl
echo '{"type":"verdict","sha":"<sha from the commit event>","verdict":"rejected","reasons":["test"],"currentBest":20}' \
  > /tmp/artifacts/artifacts/spike/cc/inbox/00000001.json
```

Check: a commit with `Ratchet-Candidate: true` appears on the branch, the agent is resumed with the verdict text,
`pause`/`resume`/`user`/`stop` messages (same inbox directory, next sequence number) behave as described in
`runRunnerLoop`, `kill -TERM` ends the turn and writes `done.json`.

### 3. Through SSM and S3

Launch the runner AMI with the agent identity, put the config with `SsmRunnerControl.start` (it uploads `config.json` and
runs `setsid nohup env ... ratchet-runner` via `AWS-RunShellScript`), then drive it from the app or a REPL. Check: the
SSM command returns quickly (the runner is detached), `events/` objects appear every ~2 s, inbox messages are consumed in
order within a poll interval, the IAM boundary allows exactly `artifacts/<opt>/<agent>/*` (a write to another agent's
prefix must fail), `stop()` terminates the process, and the kill switch (deny-all role) stops an agent mid-turn with no
further S3 writes.

### 4. Both agents on the demo

Run `kind: claude-code` and `kind: codex` against the demo fork with a real bench; compare with the scripted `cc`/`cx`
runs. Record cost per verified win.

## Flags and behaviours to verify

I wrote these from memory of the vendors' public documentation and could not run the CLIs. Treat every row as a hypothesis.

### Claude Code (`claude`)

| Assumed | Where | What to check |
|---|---|---|
| `claude -p --output-format stream-json --verbose` emits one JSON object per line; `--verbose` is required with stream-json | `buildArgs` | The CLI errors without `--verbose`? Does it print anything non-JSON on stdout (the parser tolerates and counts it)? |
| Prompt on **stdin** works with `-p` | `promptVia: 'stdin'` (default) | If not, set `promptVia: 'arg'`; the prompt is then passed after `--` |
| `--permission-mode acceptEdits` plus `--allowedTools Bash,Read,Edit,Write,Glob,Grep` is enough for unattended edit + test + commit | defaults | Tool lists may need patterns (`Bash(git:*)`); is a denied tool silent or fatal? Consider `bypassPermissions` on the sandboxed runner (`permissionMode`). `--allowedTools` is variadic in some versions, which is why the list is one comma-joined argument |
| `--resume <session_id>` continues the conversation with a new user message on stdin | `buildArgs(spec, id)` | Does the `system/init` line carry the same `session_id`, or a new one? The adapter updates `session.id` in place either way. Does an unknown id exit non-zero with a message on stderr (tested as such)? Is the transcript still resumable after the process group was SIGTERMed mid-turn? |
| `--max-turns N`, `--model M`, `--append-system-prompt` | `buildArgs` | Flag names/values; `error_max_turns` result is treated as a normal end of turn |
| `result` line has `total_cost_usd` and `usage.{input_tokens,output_tokens,cache_creation_input_tokens,cache_read_input_tokens}`; `subtype`, `is_error`, `result`, `num_turns` | `ClaudeStreamParser.resultLine` | Is cost per invocation or cumulative for the session? (We assume per invocation; if cumulative, spend is over-counted on every resume.) |
| `assistant.message.content[]` blocks `text` / `tool_use` / `thinking`; `user.message.content[]` `tool_result` | parser | Sub-agent (`Task`) tool traffic and `parent_tool_use_id` are not distinguished |
| `claude --version` exits 0 | `preflight` | |
| `system/init.apiKeySource` shows which credential was used | event `data` | Use it to assert the box is on the API key and not a stray subscription login |

### Codex (`codex`)

| Assumed | Where | What to check |
|---|---|---|
| `codex exec --json` writes JSONL events: `thread.started{thread_id}`, `turn.started`, `item.started/updated/completed{item}`, `turn.completed{usage}`, `turn.failed{error}`, `error{message}` | `CodexStreamParser` | Event and item names drift between releases (`agent_message`, `command_execution`, `file_change`, `reasoning`, `mcp_tool_call`, `web_search`, `todo_list`); unknown ones are counted in `unknownTypes` |
| Prompt via stdin using the `-` placeholder | `buildArgs` + `turn` | Version-dependent; fall back to `promptVia: 'arg'` |
| Resume: `codex exec resume <thread_id> --json ... -` | `buildArgs(spec, id)` | Syntax, and whether `--json`, `--skip-git-repo-check`, `--model` and `-c` are accepted after `resume` (some versions did not accept `--sandbox` there, which is why sandbox is set with `-c`). Also check there is no `--last` ambiguity when two agents share a `CODEX_HOME` (they should not: one runner per agent) |
| `-c sandbox_mode="workspace-write"` and `-c sandbox_workspace_write.network_access=true` | defaults | Key names and TOML quoting; whether network inside the sandbox is really needed (pnpm, aws, profiling) or `danger-full-access` is the pragmatic choice on a boundary-fenced instance |
| `--skip-git-repo-check`, `--model` | `buildArgs` | |
| No approval prompts in `exec` | none | Confirm it never blocks waiting for approval; otherwise add `--ask-for-approval never` / `-c approval_policy=never` via `configOverrides` |
| `turn.completed.usage` = `input_tokens` (including cached), `cached_input_tokens`, `output_tokens`; **no dollar cost** | `turnCompleted` | We estimate cost only when `pricing` is passed; the budget guard otherwise cannot see Codex spend. Set `pricing` from the current price list |
| `OPENAI_API_KEY` is read, or `CODEX_API_KEY` for `exec` | adapter sets both from `OPENAI_API_KEY` | Which one is honoured; whether a prior `codex login` state on the AMI overrides them |
| Exit code non-zero after `turn.failed` | `turn` | Either way the adapter treats `turn.failed` as a failed turn |

### Cross-cutting

- **Process groups.** `nodeSpawner` uses `detached: true` and kills `-pgid`. Verify on the AMI that `claude` and `codex` do
  not `setsid` their tool subprocesses out of the group (check `ps -o pid,pgid,sid,cmd` during a turn). If a vendor CLI
  daemonizes anything, abort will leave it behind; the reaper and kill switch are the backstop.
- **Usage is per turn.** Both adapters report usage once, at the end of a turn (`result` / `turn.completed`). An aborted or
  crashed turn reports none, so spend is under-counted by the partial turn. The loop therefore enforces the budget
  **between** turns: the turn that crosses it is still submitted and judged, then the loop stops (`reason: "budget"`).
- **Budget overshoot** is bounded by one turn's cost. Use `maxTurns` (`RunnerLoopDeps.maxTurns`) to bound it.
- **Event volume.** Tool results are truncated to 300 characters in events; full transcripts are not stored. If the Live
  agents screen needs them, tee raw stdout to the artifacts prefix in the adapter.

## Terms and authentication: API key vs subscription

The runner authenticates with **API keys only**: `ANTHROPIC_API_KEY` for Claude Code, `OPENAI_API_KEY` for Codex. The
adapters' preflight fails without them (`allowSubscriptionAuth: true` disables the Claude check). That is a deliberate default, not a legal
conclusion. Before M3 (open question 1 in V0_ARCHITECTURE.md), someone must read the **current** terms and decide:

- Whether a user's Claude Pro/Max login, or a ChatGPT-plan Codex login, may drive unattended, multi-hour, programmatic runs
  from cloud instances operated by a third-party tool. Consumer-plan terms and usage limits are written for interactive
  individual use; automated use at this scale is usually expected to go through the API or an enterprise agreement.
- Whether keys can be placed on a machine the user's tool launches on their behalf, and what the vendors' policies say
  about reselling or relaying access.
- Rate limits and concurrency: two agents plus experiments at once may hit API tier limits (watch for 429s: Codex surfaces
  them as `turn.failed`, Claude Code as an `is_error` result; the loop retries a crashed turn up to three times).
- Data handling: the benchmark repo and traces are sent to the model vendors; check the user's data-retention settings.

Record the outcome in this file when known. Until then: API keys, stored in SSM SecureString, readable only by the agent's
role, deleted at teardown.

## Proposed port changes (not made; `packages/core` is out of scope for this worker)

1. **`RunnerMessage` verdict should carry the best ref.** Add `bestRef?: string` (or `bestSha`) to the `verdict` message
   so the runner does not rely on the `ratchet/best` convention. Today the runner fetches `ratchet/best` from the remote.
2. **Candidate `files`.** The `commit` event carries `data.files` (changed paths relative to `refs/ratchet/accepted`) so the
   engine can pre-check protected paths cheaply. If the engine wants this typed, add an `AgentEvent` variant for it.
3. **`AgentEvent.type` could include `'tool-result'`.** Tool results are currently `type: 'tool'` with `data.phase`.
4. **`RunnerControl.start` return**: returning the handle's `instanceId` for local mode is unused; fine as is.
5. **`RunnerLoopOptions`** would benefit from optional `bestRef`, `maxIdleTurns` and `model` fields instead of the
   `RunnerLoopDeps` extension used here.
