# Market research — is this a category? (2026-10-30)

**Verdict: emerging and crowding fast.** "Agent loops against a metric" (autoresearch /
"the Karpathy loop") is commodity; the *verified* layer — noise floor, sealed workload,
tamper-proof re-bench, cost per verified improvement, in the customer's cloud — has no
documented equivalent.

## Signals
- Karpathy autoresearch (Mar 2026), viral OSS: https://thenewstack.io/karpathy-autonomous-experiment-loop/ · https://github.com/webfuse-com/awesome-autoresearch
- Shopify Liquid: 53% faster via autoresearch, author calls it "probably somewhat overfit": https://github.com/shopify/liquid/pull/2056 · https://shopify.engineering/autoresearch
- Reward hacking highest on perf tasks (4.7x of a 5.1x "speedup" from editing the benchmark): https://arxiv.org/pdf/2605.02964 · https://arxiv.org/pdf/2603.11337v1 · https://arxiv.org/html/2605.21384v1
- Evolutionary frameworks often fail on private scores: https://arxiv.org/html/2605.20086v1
- AlphaEvolve GA on Google Cloud (Jul 2026), customer-written evaluator: https://www.infoq.com/news/2026/07/alphaevolve-generally-available/

## Closest comparables

| Who | Optimizes | Approach | Verification | Deployment | Pricing |
|---|---|---|---|---|---|
| [CodSpeed Optimization Agent](https://codspeed.io/optimization-agent) | Multi-language perf | Agent + MCP, bare-metal, PRs | Strong measurement; no sealed workload / anti-hack | SaaS + GitHub | Agent: contact sales |
| [Codeflash](https://www.codeflash.ai/pricing) | Python functions | Own pipeline; Claude Code plugin | Behavioural equivalence via tests | SaaS / GitHub app | Free; $20/user/mo |
| [Weco (AIDE)](https://docs.weco.ai/) | Any scalar metric | Tree search, parallel branches | User's eval only | Eval on user HW | n/a |
| [TurinTech Artemis](https://www.enterprisetimes.co.uk/2025/03/21/turintech-launches-artemis-to-resolve-ai-driven-technical-debt/) | Enterprise code | Multi-agent + GA | Claims validation | Enterprise | n/a |
| [AlphaEvolve](https://blog.google/innovation-and-ai/infrastructure-and-cloud/google-cloud/alphaevolve-on-cloud/) | Algorithms, kernels | Gemini evolutionary search | Customer evaluator | Google Cloud | n/a |

Also: OpenEvolve, ShinkaEvolve, CodeEvolve; SkyPilot autoresearch scaling (https://blog.skypilot.co/scaling-autoresearch/);
Bencher / Nyrkiö (statistical CI benchmarking, no agents); orchestrators Conductor, Factory, Cursor, Devin.
Funding flows mostly to orchestration and hyperscalers; pure perf optimizers are thinly funded (Weco $8M seed, TurinTech $20M).

## Differentiated vs table stakes
- Table stakes: agent loop, PR output, benchmark keep/revert, customer-side eval.
- Ratchet's edge: sealed hidden trace, gain must beat measured noise, tamper-proof re-bench of every commit,
  cost per verified improvement, budget/deadline campaigns, harness-agnostic, runs in customer's AWS.

## Risks
1. Bundling by CodSpeed / Codeflash / Google / orchestrators.
2. OSS commoditization (autoresearch + SkyPilot).
3. Customers must supply a good benchmark + trace — onboarding burden.
4. No published willingness-to-pay for "cost per verified improvement".

## Positioning
1. Lead with trust: "Performance gains you can merge."
2. Outcomes in your cloud: cost per verified improvement; neutrality vs SaaS / Google lock-in.
3. Be the verification layer: open spec + CLI others' outputs run through; partner with CodSpeed/Bencher/Nyrkiö.

Unverified items: several prices, funding amounts and verification claims; see the research thread.

---

# Round 2 — generic "verified business-metric improvement" platforms

**Verdict: partly unique.** The core loop (agent proposes → user eval scores → keep
winners) is commoditizing fast across hyperscalers, seed startups, YC and OSS. Not
found anywhere: a sealed-holdout, tamper-proof evaluator *as the product*; budget-capped
campaigns with $ value per $ spent as the headline; outcome/gain-share pricing outside
FinOps; packs spanning perf → forecasting.

## Closest five

| Player | Domain | Rigor | Deploy | Pricing | Stage |
|---|---|---|---|---|---|
| [hiloop / Autoresearch Inc](https://www.ycombinator.com/companies/autoresearch-inc) | ML/post-training, "any measurable" | "Statistical verification" (paired confirm runs); no holdout detail | Hosted or your cloud | Unknown | YC S26, 3 people |
| [Weco](https://www.weco.ai/platform) | Any scripted metric (ML, kernels, prompts) | Your eval script | CLI + hosted | Credits / BYOK / enterprise | $8M seed |
| [AlphaEvolve on GCP](https://blog.google/innovation-and-ai/infrastructure-and-cloud/google-cloud/alphaevolve-on-cloud/) | Algorithms, logistics, runtimes | Your scorer | Google Cloud | Unpublished | GA |
| [Applied Compute](https://www.appliedcompute.com/) | Bespoke RL agents | Customer graders | VPC option | Services + compute | ~$160M raised |
| [TensorZero Autopilot](https://www.tensorzero.com/blog/automated-ai-engineer/) | LLM-app metrics | A/B on prod feedback | Self-hosted | Unpublished | $7.3M seed |

Also notable: [Autolab](https://www.autolab.ai/autoresearch.html), [Autoheal](https://venturebeat.com/orchestration/autoheal-wants-to-manage-the-work-ai-coding-agents-leave-behind-claiming-cost-reductions-of-up-to-30-per-task) (evaluator-gated PRs, per-session pricing),
[Hightouch AI Decisioning](https://hightouch.com/docs/ai-decisioning/agents) (best holdout rigor, marketing only),
[Databricks Agent Bricks](https://www.databricks.com/blog/introducing-agent-bricks), [SageMaker agent skills](https://aws.amazon.com/about-aws/whats-new/2026/10/bring-sagemaker-ai-expertise-to-your-coding-agent/),
Sedai / Cast AI / ProsperOps (FinOps, gain-share pricing), Nixtla / o9 / Blue Yonder (forecasting suites).

## Pack crowding
- ML model quality — most crowded (hiloop, Weco, Autolab, Applied Compute, hyperscalers).
- Code perf — crowded (CodSpeed, Codeflash, TurinTech, AlphaEvolve, Weco).
- FinOps — crowded and mature, but runtime-config, not code/model changes.
- Pricing/marketing — incumbents with RL decisioning (Hightouch, Aampe/MoEngage, Optimizely).
- **Forecasting — most open** for a backtest-gated "improve your own pipeline" loop (absence of evidence, not proof).

## Threats
1. Hyperscaler bundling (AlphaEvolve, free SageMaker skills, Databricks).
2. Weco (first mover, logos, harness-agnostic).
3. hiloop (same pitch incl. "verified" and "your cloud").
4. Coding agents + OSS autoresearch skill = 80% for free.
5. Incumbents moving down-stack (Datadog+Eppo, Sedai, Hightouch).

## Implication
The agent loop is not a moat. Differentiate on **verification + ROI accounting as the
product** (sealed holdouts, noise floor, tamper-proof evaluator, $ value per $ spent,
outcome pricing), start in perf, move to **forecasting** next.
