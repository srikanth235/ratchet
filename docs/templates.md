# Evaluator templates

The simplified model (docs/SIMPLIFIED_MODEL.md) has one way into a repo: a **template**. It is a `ratchet.yaml` fragment,
the evaluator's real files and a `RATCHET.md` brief. "Packs" are no longer a user-facing concept: they are the template
`category` (`perf`, `forecasting`, `llm`), and the older docs in `docs/packs/` describe the categories.

## The gallery

| Template id | Category | Measures | Holdout | Machine | Default objective |
|---|---|---|---|---|---|
| `http-latency` | perf | p50/p95/p99 of an open-loop load test, error rate | hidden request mix | c7i.2xlarge | minimize `http_req_duration.p95` |
| `cli-cold-start` | perf | wall time of invocations (hyperfine when installed) | hidden invocations | c7i.large | minimize `cold_start_ms.p50` |
| `test-suite-time` | perf | duration of the test command | none | c7i.2xlarge | minimize `test_suite_ms` |
| `build-size` | perf | build seconds, bundle KB | none | c7i.xlarge | minimize `bundle_kb` |
| `memory` | perf | peak RSS of the workload's process tree | hidden workload | c7i.xlarge | minimize `peak_rss_mb` |
| `forecast-backtest` | forecasting | WAPE / MAPE / bias of a rolling-origin backtest | most recent periods | c7i.xlarge | minimize `wape` |
| `llm-eval` | llm | score, cost and tokens per call, error rate | hidden eval set | c7i.large | maximize `score` |
| `demo-search`, `demo-cli`, `demo-forecast` | perf / perf / forecasting | "Try a demo": metadata only, point at `examples/demo-*`, run in simulation with no setup | | | |

Every template ships real, dependency-free scripts (`bench/*.mjs`, `eval/*.mjs`) in the style of `examples/*/bench`:
they print one JSON object on stdout (the last one wins), read the sealed holdout from `RATCHET_TRACE` when it is set, and
start candidate code through `$RATCHET_CANDIDATE_EXEC` when the bench provides it (so the candidate cannot read the holdout).
A JSON config next to each script (`bench/http-latency.config.json`, ...) holds what the repo has to fill in.

## Use a template

CLI:

```sh
ratchet init --list-templates
ratchet init --template http-latency [--metric <path>] [--direction minimize|maximize] [--value-usd <n>] [--dir <dir>] [--force] [--dry-run]
ratchet init --pack perf        # legacy alias of --template (perf = cli-cold-start, forecasting = forecast-backtest)
```

Desktop / programmatic (core exports, EngineApi methods):

```ts
import { listTemplates, getTemplate, renderTemplate, buildTemplateProposal } from '@ratchet/core';

listTemplates({ kind: 'template' });                  // TemplateInfo[]: the gallery (kind 'demo' for "Try a demo")
getTemplate('http-latency');                          // TemplateInfo | undefined (legacy pack names accepted)
renderTemplate('http-latency', { metric, valueUsd }); // { info, options, files: [{ path, content }] }  (pure)
buildTemplateProposal('http-latency');                // + branch, commit message, PR title and body        (pure)

engine.listTemplates();                               // Promise<TemplateInfo[]>
engine.proposeTemplate({ repoUrl, baseRef, templateId, overwrite? });
//   -> Promise<{ prUrl, branch, sha, templateId, files }>
//   commits the files on ratchet/template-<id> via GitHost.commitFiles and opens the PR via openPullRequest(shas: [])
```

`TemplateInfo` is `{ id, kind: 'template' | 'demo', category, title, description, measures, holdout: string | null, machine,
evaluatorKind, objective: { metric, direction, unit }, defaultValueUsd, files: string[], example?: string }`.
`proposeTemplate` refuses (`EngineError` `invalid_state`) when `ratchet.yaml` or another template file already exists on
`baseRef`, unless `overwrite` is true; it refuses demos (`invalid_input`) and a git host without `commitFiles` (`unavailable`).
The PR description states the objective, the holdout and the machine, a table of the files added, what to fill in, and how to
run a baseline (`<evaluator command>`, `ratchet validate`, `ratchet baseline`).

## In ratchet.yaml

```yaml
evaluator:
  template: http-latency   # resolved when the config is parsed: run, kind, machine, default guardrails
```

`run:` still works and wins when both are present. The template's files (`bench/**`, `eval/**`, `data/**`) are protected
automatically (core `effectiveProtected`), together with `ratchet.yaml`, lockfiles and install config.

## Adding a template

1. `templates/<id>/` (repository root): `RATCHET.md` (placeholders `{{metric}}`, `{{direction}}`, `{{verb}}`) and the files at
   their repo-relative paths. They are embedded into `ratchet-core` at compile time (`include_dir!`); there is nothing to generate.
2. An entry in `crates/ratchet-core/src/templates/registry.rs` (metadata, `run`, default guardrails, `protect` globs).
3. Add the template id to the id list in `crates/ratchet-core/tests/templates.rs` and a script test there: every generated config
   must pass `parse_config`, and the evaluator must run and print the objective metric. `the_embedded_files_are_in_sync_with_the_template_directory`
   fails when the embedded copy and `templates/` differ (rebuild).
