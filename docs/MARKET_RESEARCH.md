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
