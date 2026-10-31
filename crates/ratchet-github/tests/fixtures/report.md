# Ratchet result: Demo search p95

**http_req_duration.p95: 412 → 180 (−56.3%)** — 2 verified wins, $8.00 spent, $4.00 per verified win.

Repo `acme/search` · base `main` · minimize · status draft

## Percentiles

| Stat | Baseline | Best | Change |
| --- | --- | --- | --- |
| value (http_req_duration.p95) | 412 | 180 | −56.3% |
| mean | 260 | 160 | −38.5% |
| p50 | 240 | 150 | −37.5% |
| p95 | 412 | 180 | −56.3% |
| p99 | 690 | 300 | −56.5% |

Noise floor: 0.02

## Verified wins

| # | Agent | SHA | Change | Bench | Confirm | Step | Cost |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | claude-code | `aaaaaaaa` | Cache tokenizer | 300 | 305 | −27.2% | $1.50 |
| 2 | codex | `bbbbbbbb` | Index postings \| fast | 180 | 185 | −40.0% | $3.75 |

_Cost: agent spend since that agent’s previous verified win._

## Candidates

| Verified | Rejected | Failed confirm | Blocked | Pending |
| --- | --- | --- | --- | --- |
| 2 | 1 | 1 | 1 | 0 |

## Agents

| Agent | Branch | Candidates | Verified | Spend |
| --- | --- | --- | --- | --- |
| claude-code | `ratchet/opt1/claude` | 3 | 1 | $3.75 |
| codex | `ratchet/opt1/codex` | 2 | 1 | $3.75 |

## Spend

| Category | USD |
| --- | --- |
| agent_runner | $3.00 |
| experiment | $2.25 |
| bench_runner | $0.50 |
| tokens | $2.25 |
| **total** | **$8.00** |

$1.50 reconciled, $6.50 estimated. Cost per verified win: $4.00.

## Reproduce

```sh
git fetch origin main ratchet/opt1/claude ratchet/opt1/codex
git checkout bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
k6 run bench/k6/search.js
```
