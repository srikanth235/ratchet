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

### Golden (differential) tests: `tests/golden.rs`, fixtures from the TypeScript

The fixtures were produced by running the TypeScript with `tsx` on seeded inputs. The generator (`generate.mjs`) was deleted with the
TypeScript at the cut-over, so they are frozen expectations (it is at `fc503b1:crates/ratchet-env/tests/golden/generate.mjs`;
see docs/CUTOVER.md).

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

## Drafters, approve, tree reader, engine and daemon

| TS | Rust | TS tests | Rust tests |
|---|---|---|---|
| `engine/environment/drafter.ts` (`AgentEnvironmentDrafter`) | `src/drafter.rs` | `drafter.test.ts`: 16 `it`s | `tests/drafter.rs`: 18 (the same cases, a few folded into one fn, plus the begin/run halves and a git host that cannot commit) |
| `engine/environment/mock.ts` (`MockEnvironmentDrafter`, `scopeFromGoal`) | `src/mock.rs` | `mock.test.ts`: 15 (incl. the PR text) | `tests/mock.rs`: 15 (the "tampered check" case needs a mutable record in TS; Rust records are copies, so it is covered by `drafter.rs::approve_refuses_a_draft_whose_checks_did_not_pass`) |
| `engine/environment/approve.ts` (`commitDraft`, PR text) | `src/approve.rs` | in `drafter.test.ts` / `mock.test.ts` | in `tests/drafter.rs` / `tests/mock.rs` |
| `engine/environment-files.ts` (`readEnvironmentTree`, `readEnvironment`) | `src/tree.rs` | in `apply.test.ts` | exercised by `ratchet-engine/tests/engine/environment_apply.rs` |
| `engine/environment/apply.test.ts` (engine applies the approved environment, locks it) | `ratchet-engine/src/engine.rs::environment_of`, `sandbox_auto.rs` | 10 `it`s (some `it.each`) | `ratchet-engine/tests/engine/environment_apply.rs`: 10 (the `it.each` rows are looped) |
| `aws/sandbox/environment.ts`, `environment-check.ts`, drafted stage of `tofu.ts` / `mock.ts` | `ratchet-sandbox/src/sandbox/environment*.rs` | `environment.test.ts`: 28 | `tests/suite/sandbox_environment.rs`: 27 |

Deliberate differences of the drafter: `draft` / `revise` are `begin_*` (synchronous, record the request as `drafting`) plus
`run_*` (the agent's turn) so the daemon can answer a `POST` at once; `draft()` / `revise()` are the two together, as in
TypeScript. `DraftEnvironmentInput.id` lets the caller pick the draft id. The agent is an `AgentAdapter` handed in by a
factory (`ratchet-agents::local_adapter_for` in the daemon) and `tofu plan` is an `EnvironmentChecker` (`ratchet-sandbox`),
which keeps this crate dependent on `ratchet-core` only. Why a crate and not a module of `ratchet-engine`: `ratchet-sandbox`
re-validates an approved environment when it applies it, and the engine depends on `ratchet-sandbox`.
