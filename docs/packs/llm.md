# Category: llm (LLM-backed tasks)

A template *category* (formerly "pack"; see docs/templates.md). It makes a task that calls a language model score higher, or
cost less per call, against a deterministic eval set the agents cannot edit.

| | |
|---|---|
| `pack` | `llm` (informational) |
| Template | `llm-eval` |
| Objective | `score` (mean case score in 0..1), maximize; `cost_per_call_usd` and `tokens_per_call` are reported too |
| Evaluator `kind` | `eval` |
| Holdout | a hidden eval set in the same format (`RATCHET_TRACE`), different cases of the same kind |
| Guardrails | `error_rate: "< 0.05"` (calls that threw), plus your test command |
| Machine | c7i.large |

```sh
ratchet init --template llm-eval
node eval/llm-eval.mjs      # { score, cost_per_call_usd, tokens_per_call, error_rate, p95_latency_ms, n }
```

Files written: `ratchet.yaml`, `RATCHET.md`, `eval/llm-eval.mjs` (the protected evaluator), `eval/dataset.jsonl` (the visible
cases: `{ "input", "expected", "match": "exact" | "contains" | "regex" | "number" }`, one per line) and `src/task.mjs`, the
code the agents improve: `run(input)` returns the answer or `{ output, cost_usd, tokens }`. The placeholder is a
deterministic keyword classifier so the template runs end to end with no network and no key; replace it with your real call.

Scoring is deterministic on purpose, so the same candidate scores the same twice and the noise floor stays small. To grade
with a judge model, add a scorer to `eval/llm-eval.mjs` and pin the judge's model, prompt and temperature: the file is
protected, so candidates cannot change how they are graded. Overfitting to the visible cases is what the hidden set is for;
the brief tells agents so.
