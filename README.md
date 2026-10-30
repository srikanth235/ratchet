# Ratchet

Agents propose changes. A trusted benchmark decides. Only verified wins ratchet forward.

- Architecture: [docs/V0_ARCHITECTURE.md](docs/V0_ARCHITECTURE.md)
- Plan: [docs/PLAN.md](docs/PLAN.md)

## Layout

```
packages/core     engine: ratchet.yaml schema, ratchet rule, ledger, agents, AWS
packages/cli      `ratchet` CLI
packages/ui       Midnight Brass design tokens
packages/desktop  Electron + React app
infra/            OpenTofu modules + sandbox contract
examples/         demo repos for benchmarks
```

## Develop

```bash
pnpm install
pnpm build && pnpm typecheck && pnpm test
```
