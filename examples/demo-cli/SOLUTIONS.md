# Planted cold-start problems (protected: hidden from agents)

Seven problems, each fixable without changing a byte of output (`tests/golden/*.out`). They overlap in what
they cost per invocation: after the first fixes the rest are a smaller share of a smaller total. Typical
effect on this development box (4 vCPU, p50 of the 11 visible invocations, unloaded): about **350 ms ->
about 85 ms**; node itself is ~35 ms of that.

| # | Problem | File | ~ms saved |
|---|---|---|---|
| 1 | message catalogs | `src/messages.ts` | 60 |
| 2 | spelling dictionary | `src/dictionary.ts` | 50 |
| 3 | time-zone table | `src/tz.ts` | 80 |
| 4 | plugin scan | `src/plugins.ts`, `src/config.ts` | 40 |
| 5 | `tput` shell-outs | `src/terminal.ts` | 20 |
| 6 | eager context (store, plugins, terminal) | `src/context.ts` | 15 to 30 |
| 7 | all commands and the update checker imported eagerly | `src/commands/index.ts`, `src/cli.ts`, `src/update-check.ts` | 20 |

## 1. Every message catalog is read and compiled at import (`messages.ts`)

`data/messages/*.json` holds ~4,000 ICU-style messages in 16 files: the handful the commands print, and a long
tail (error codes, validation rules, deprecations, wizard prompts, telemetry, plugin diagnostics). The module
reads all of them and compiles every message to a closure when it loads.
**Fix:** load a catalog when the first key of its area is asked for (keys are namespaced by file name), or
compile messages lazily. Only `errors` and the area of the running command are ever needed.

## 2. The "did you mean" dictionary is built at import (`dictionary.ts`)

7,000 pseudo-words are generated and indexed by character trigrams at module load, though only a mistyped
command or an empty `search` uses them. **Fix:** build `vocabulary` and `index` on first use.

## 3. A table of every IANA zone is built at import (`tz.ts`)

For each of ~420 zones a `Intl.DateTimeFormat` is constructed and asked for six offsets, to answer questions
about one zone (the configured one, or `--tz`). Constructing `Intl.DateTimeFormat` is the expensive part.
**Fix:** describe a zone when it is asked for (memoize), validate names with a single formatter construction.

## 4. Plugin discovery opens every package (`plugins.ts`, `config.ts`)

To find `taskr-plugin-*` and `@taskr/plugin-*` it reads and parses the `package.json` of every package in every
`node_modules` above the working directory (~1,500 in the benchmark workspace). The directory name is the package
name: only matching directories need opening. Related: config discovery climbs to the filesystem root and
cuts the result at `"root": true` afterwards; stop climbing at the root file.

## 5. Two `tput` child processes (`terminal.ts`)

`tput cols` and `tput colors` are run through a shell on every start (a fork+exec each). `COLUMNS`, the tty
width and `TERM` give the same answers.

## 6. The context is resolved eagerly (`context.ts`)

Every command, including `help`, `version` and unknown ones, scans for plugins, asks the terminal, and reads,
validates and indexes the whole task file. **Fix:** resolve those members on first use (getters).

## 7. Everything is imported (`commands/index.ts`, `cli.ts`, `update-check.ts`)

All nine command modules (`export` compiles ~100 escaping regexes at load) and the opt-in update checker
(`node:https`, `node:crypto`, `node:zlib`) are imported although one command runs. **Fix:** import the command by
name; import the update checker only when `TASKR_UPDATE_CHECK=1`.

## Not planted, but real

- `module.enableCompileCache()` / `NODE_COMPILE_CACHE` (node 22): caches compiled code between starts. The benchmark
  starts every run with an empty cache directory, so it cannot help here; on a real install it would.
- Bundling the CLI into one file, or a V8 startup snapshot: allowed in principle, but "no bundlers" is in the brief.

## Verifying a fix

`pnpm test` must stay green (it checks the golden outputs). `pnpm bench:spawn` for the effect; expect noise of
a few percent unloaded and much more on a busy box.
