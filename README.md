# Ratchet

Agents propose changes. A trusted benchmark decides. Only verified wins ratchet forward.

- Architecture: [docs/V0_ARCHITECTURE.md](docs/V0_ARCHITECTURE.md)
- Product model (settings, harness detection, templates, invisible sandbox): [docs/SIMPLIFIED_MODEL.md](docs/SIMPLIFIED_MODEL.md)
- Plan: [docs/PLAN.md](docs/PLAN.md)

## Layout

```
packages/core     engine: ratchet.yaml schema, ratchet rule, ledger, agents, AWS
packages/cli      `ratchet` CLI
packages/ui       Midnight Brass design tokens
packages/desktop  Electron + React app
packages/e2e      Playwright end-to-end specs and design-conformance tooling
infra/            OpenTofu modules + sandbox contract
examples/         demo repos for benchmarks
```

## Develop

```bash
pnpm install
pnpm build && pnpm typecheck && pnpm test
```

## Run the desktop app

```bash
pnpm --filter @ratchet/desktop dev            # electron-vite dev server
```

1. **Try a demo first:** New optimization -> Repo -> "Try a demo" -> Review -> Start. It runs in simulation (scripted agents, mock AWS,
   a local evaluator) and needs no account, no CLI and no setup.
2. **Real runs:** Settings has three rows. *GitHub*: "Sign in with GitHub" (set `RATCHET_GITHUB_CLIENT_ID` to your OAuth app, device flow
   enabled). *AWS*: "Connect AWS" and pick a profile from `~/.aws` (region defaults to `us-east-1`; a repo can override it in
   `ratchet.yaml`). *Agents*: the Claude Code and Codex CLIs installed on this machine are detected, with their sign-in status.
   Then pick a repository (or a local folder with a GitHub `origin`). The repo owns `ratchet.yaml`; if it has none, add a
   template through a pull request (also under Benchmarks -> Templates).
3. Agents run on this Mac by default (the app keeps it awake during a run) or in the cloud; the evaluator always runs in an automatic
   sandbox in your AWS account that is removed after the last run, with a receipt on Result and Environments.

End-to-end tests (Playwright under Xvfb, against the built app and fakes for GitHub, the agent CLIs and `~/.aws`):

```bash
pnpm --filter @ratchet/e2e test:e2e         # SKIP_BUILD=1 reuses packages/desktop/out; E2E_FILTER=03 runs one spec
```
