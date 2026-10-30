# Ratchet brief: {{verb}} `{{metric}}` of the LLM task

**Goal:** {{direction}} `{{metric}}`, the mean score of `run()` in `src/task.mjs` over the eval set, reported by
`eval/llm-eval.mjs` (it also reports `cost_per_call_usd` and `tokens_per_call`). A change counts only if the trusted
evaluator, which you cannot edit, confirms it, then again on a hidden eval set you do not see. Your own measurements are
for guiding your work.

## Rules

- Do **not** modify protected paths: `eval/**`, `ratchet.yaml`, lockfiles and install config. A candidate that touches
  them is blocked. That includes the eval set: the cases in `eval/dataset.jsonl` are the visible sample of a larger set.
- **Don't overfit to the visible cases.** Quoting their answers, or special-casing their wording, scores well here and
  fails on the hidden set. Improve the prompt, the examples, the output format, the parsing and the model settings.
- Keep an eye on cost: a gain that doubles the cost per call is a trade-off, not a free win. State it in the commit message.
- Don't add dependencies: the benchmark machine has no network and installs from the base lockfile.
- One idea per commit, with a message saying what you measured. To submit a candidate, add the trailer
  `Ratchet-Candidate: true` to the commit.

## Workflow

```sh
npm test                   # must pass before and after
node eval/llm-eval.mjs     # prints { score, cost_per_call_usd, tokens_per_call, error_rate, n }
EVAL_LIMIT=5 node eval/llm-eval.mjs   # a quick look at the first cases
```

TODO: describe the task, the model and key it uses, and the failure patterns you have already seen.
