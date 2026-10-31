# ratchet-env parity with the TypeScript implementation

Spec: `packages/core/src/environment/*` and `packages/engine/src/environment/*` (the TypeScript stays until cut-over).

## Pure modules (fork A)

| TS file | Rust | TS tests | Rust tests |
|---|---|---|---|
| `core/environment/hcl.ts` | `src/hcl.rs` | `hcl.test.ts`: 6 | 6 unit tests (`src/hcl.rs`) |
| `core/environment/cost.ts` | `src/cost.rs` | `cost.test.ts`: 10 | 10 unit tests (`src/cost.rs`) |
| `core/environment/environment.ts` | `src/environment.rs` | `environment.test.ts`: 11 (6 are about the module ref, the hash, the sandbox id, `draftPassed`) | 6 unit tests (`src/environment.rs`) |
| `core/environment/config-yaml.ts` | `src/config_yaml.rs` | covered by `environment.test.ts` | 2 unit tests (all edit shapes), plus `tests/locked.rs` |
| `environment.test.ts` (config + locked paths: the other 5 of the 11) | `tests/locked.rs` | 5 | 5 |
| `core/environment/brief.ts` | `src/brief.rs` | `brief.test.ts`: 7 | 7 unit tests (`src/brief.rs`) |
| `core/environment/validate.ts` | `src/validate.rs` | `validate.test.ts`: 63 vitest cases (the `it.each` rows count one each) | 36 tests in `tests/validate.rs`: every `it` and every `it.each` row, the rows looped inside one fn |
| `engine/environment/samples.ts` | `src/samples.rs` | used by `mock.test.ts` / `apply.test.ts` | covered by `tests/golden.rs::samples_match_typescript` (12 cases) |

Total for these modules: 97 TypeScript cases (`vitest run src/environment`) against 31 unit + 36 validate + 5 locked = 72
Rust tests that fold the `it.each` rows into loops, plus the golden differential tests below.

### Golden (differential) tests: `tests/golden.rs`, fixtures from `tests/golden/generate.mjs`

The fixtures are produced by running the TypeScript with `tsx` on seeded inputs (regenerate: see the header of
`generate.mjs`; `@ratchet/core` must be built because the engine's `samples.ts` imports it from `dist`).

| What | Cases |
|---|---|
| `validateEnvironment` on the three samples and mutations of them (every check id, label, ok, detail; machines; resources; cost; problems) | 560 |
| `estimateMachinesCost` (unpriced types, volumes, custom prices, time limits) | 400 |
| `parseHcl` + `stripComments` (blocks, attrs in order, `raw`, `line`, errors) on the sample files, snippets and mutations | 461 |
| `parseAgentAnswer` / `formatAgentAnswer` round trips | 23 + 24 |
| `buildSetupBrief` (goal x hardware x time limit), `buildReviseBrief`, byte for byte | 80 + 4 |
| `hashEnvironmentFiles`, `environmentSandboxId`, `environmentModuleOf` | 200 + 16 + 28 |
| `applyEnvironmentToConfigYaml` / `readEnvironmentScope` | 85 + 22 |
| `sampleEnvironment` per scope and option, `SAMPLE_REASONS`, `SAMPLE_EFFORT` | 12 |

### Deliberate differences

1. **`apply_environment_to_config_yaml` edits text, it does not re-serialize.** The TypeScript uses the `yaml` Document API
   (comments survive, the rest is re-formatted). `serde_norway` cannot keep comments, so Rust replaces / appends only the
   keys of the `environment:` block in place and keeps every other byte (comments, key order, CRLF, a missing final newline).
   A flow-style block (`environment: {image: x}`) or an explicit `null` is rewritten in block style. The golden test compares
   the parsed meaning of both outputs (and that every other top-level key is unchanged), not the bytes.
2. **YAML syntax errors have the parser's wording.** `ratchet.yaml: invalid YAML (line L, column C): <message>` in the `config`
   check comes from `serde_norway` in Rust and from `yaml` in TypeScript; the golden test does not compare the part after
   the colon (nor line / column).
3. `environment:` with no value / `~`: the TypeScript `setIn` throws ("Expected YAML collection at environment"); Rust
   treats it as an empty block and sets the keys. Documents whose root is a scalar or a list are refused by both.
4. Numbers: durations and minutes are integers (`u64` / `u32`), money is `f64`. `Math.round` is reproduced exactly (halves
   round up), `toFixed` through `ratchet_core::numfmt::to_fixed`.
5. The HCL reader scans `char`s where the TypeScript scans UTF-16 units; the results are identical except for astral
   characters inside a line-length-sensitive position (not reachable by a valid draft).
6. `regex` has no look-behind: `(?<![\d.])0\.0\.0\.0\/0` and `(?<![\w.])(file|...)\s*\(` are checked by hand; `\b`, `\w`
   are ASCII (`(?-u:...)`), as in JavaScript.

## Pending (owner: main)

| TS | Rust | Status |
|---|---|---|
| `engine/environment/drafter.ts` (`AgentEnvironmentDrafter`) | `src/drafter.rs` | pending (owner: main) |
| `engine/environment/mock.ts` (`MockEnvironmentDrafter`) | `src/mock.rs` | pending (owner: main) |
| `engine/environment/approve.ts` | `src/approve.rs` | pending (owner: main) |
| `engine/environment-files.ts` | `src/tree.rs` | pending (owner: main) |
