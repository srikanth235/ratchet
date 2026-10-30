# demo-cli scripts for `ScriptedAgent`

Real patches against `examples/demo-cli` (paths relative to its root; apply with `git apply`, or from the repo root with
`--directory=examples/demo-cli`), and two scripts that play them as agent attempts. Regenerate the scripts with
`node scripts/demo-cli/build-scripts.mjs` (from `packages/agents`; build the demo first, the overfit table is recorded from
its output on the visible invocations); load them with `loadDemoCliScript('cc' | 'cx')`.

| Patch | SOLUTIONS.md item | Needs |
|---|---|---|
| `01-lazy-message-catalogs` | 1. load a message catalog on first use | none |
| `02-lazy-spelling-dictionary` | 2. build the dictionary on first use (also updates the unit test that read the eager export) | none |
| `03-lazy-time-zones` | 3. describe a zone when asked for | none |
| `04-bounded-config-and-plugin-scan` | 4. open only plugin directories; stop the config search at the root file | none |
| `05-no-tput-shell-out` | 5. terminal size from `COLUMNS` / tty / `TERM` | none |
| `06-lazy-context` | 6. store, plugins, terminal resolved on first use | none |
| `07-lazy-commands` | 7. import the command by name, the update checker on demand | none |

All seven are independent and keep `pnpm test` green in any order; `test/demo-cli-scripts.test.ts` applies the order each
script plays and runs the demo's own build and tests at the end.

## Scripts

`cc` plays 03, 01, 02, 04, 05; `cx` plays 01, 07, 02, 03, 06. Each also submits the same kinds of bad attempt:

| Bad attempt | cc | cx | Expected verdict |
|---|---|---|---|
| edits a protected file with a real fix alongside | `bench/spawn.ts` (turns on `NODE_COMPILE_CACHE` for the measurements) + the `tput` fix | skips plugin discovery and rewrites `tests/golden/{config,config-json,version-verbose}.out` to match | **blocked** (protected path) |
| overfit to the visible invocations | `src/snapshot.ts`: recorded output of the 11 visible invocations, replayed from `main.ts` before anything loads, after a hash of the installed package manifests confirms the environment (as costly as the unoptimized plugin scan) | same | big bench win (hits cost ~85 ms), then **failed confirm**: the hidden mix never hits the table, so each of its runs pays for the hash and the table on top of the normal start |
| noise-level no-op | `src/render/table.ts` (`trimEnd` -> regex) | `src/store.ts` (truthiness test -> `=== undefined`) | **rejected** |

A bad attempt is followed by `git reset --hard refs/ratchet/accepted` in the next attempt. Steps are idempotent (`skip`
markers), so two agents can share a best branch; an equivalent idea landing after the other agent's is `rejected` (the
ratchet is a race). `snapshot.ts.txt` is the generated table, kept for reference.
