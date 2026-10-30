# Real-environment test plan: trusted bench

Scope: `@ratchet/bench` (`LocalBenchRunner`, `RemoteBenchRunner`, `BenchQueue`, `evaluateCandidate`).
Unit and integration tests run the pipeline on one machine; nothing in them proves the numbers are
stable on the pinned instance, that the generated scripts survive real SSM, or that candidate code
cannot reach the hidden trace. This plan does that. Run it after `aws.md` (sandbox, identities) and
before `agents.md` / `engine.md`.

## 1. What runs where

One bench job (`BenchJob`: `baseline`, `bench` or `confirm`) is the same pipeline in both runners
(`src/pipeline.ts`); only the executor differs:

```
checkout candidate sha + bench.ref  ->  protected overlay  ->  setup  ->  command guardrails (fail fast)
  ->  bench.run x job.repeats  ->  threshold guardrails over the per-metric medians
```

| | `LocalBenchRunner` | `RemoteBenchRunner` |
|---|---|---|
| Used for | simulation mode, `ratchet bench`, development | live mode |
| Where candidate code runs | this machine, current user, current env (**not a security boundary**) | a fresh bench instance per job (`CloudCompute.launchBenchInstance`), terminated in a `finally` |
| Checkout | `GitHost.checkout(sha)` and `GitHost.checkout(bench.ref)`, `.git` removed | the instance runs `git fetch <remoteUrl> <sha>` and `<bench.ref>`, then `git archive` |
| Steps | child processes in their own process group | one generated bash script per step through `CloudCompute.runCommand` |
| Abort | `AbortSignal` kills the process groups | `AbortSignal` stops waiting and terminates the instance |

Failure model: a candidate that breaks a gate (setup, a command guardrail, a bench run, a threshold)
yields a `BenchOutcome` with a failed `GuardrailResult`; `evaluateCandidate` turns that into
`blocked`. Infrastructure problems (checkout fails, `bench.ref` has no protected files, instance never
registers with SSM, confirm job without a trace) throw `BenchError`; an abort throws an `AbortError`.

### The protected-path rule (what comes from `bench.ref`)

A path is *protected* iff it matches a glob in `ratchet.yaml` `protected:` (core's `matchesGlob`; the
remote scripts use the same glob-to-regex translation through `grep -P`, and a test checks the two agree).
Protected paths always come from `bench.ref`, never from the candidate:

1. every file or symlink in the `bench.ref` tree that is protected overwrites the candidate's version
   (and replaces whatever the candidate has there: a directory, a symlink);
2. with `pruneExtraProtected` (default **on**) every protected file the candidate has that `bench.ref`
   does not is deleted. This assumes `bench.ref` is a full snapshot (baseline plus the trusted bench),
   which is how the demo is set up; with a bench.ref holding only `bench/`, set it off or `tests/contract/**`
   would be deleted;
3. the copy never follows symlinks: an ancestor that is a symlink or file in the candidate tree is
   removed and replaced by a real directory first;
4. if `bench.ref` contains **no** protected file the job fails (`BenchError`): an empty trusted bench
   means the candidate would be scoring itself;
5. before **every** later step (setup, each guardrail, each bench run) the protected files are compared
   with `bench.ref`'s. If a build step, a test or a previous bench run modified one, it is restored, the
   step is not run, and the job ends with a failed `protected files intact` guardrail (verdict `blocked`).

Not covered, by design: the `protected` list is also checked on the diff by the engine
(`changedFiles` + `findProtectedViolations`), which is what reports *which* committed path was touched.
The overlay is the second line. It does not make an unprotected path trusted: if `bench.run` reads
`tools/load.mjs` and `tools/**` is not in `protected`, the candidate controls the bench. Review the
`protected` list against everything `bench.run` executes or reads. The setup command is candidate-
influenced (it runs `pnpm build` on candidate code), so generated output (`dist/`) is only as trustworthy
as the build; prefer a bench that runs from protected sources (k6 scripts) over a compiled one.

### Environment contract for `bench.run`

Every step gets: `RATCHET_PHASE` (`baseline|bench|confirm`), `RATCHET_SHA`, `RATCHET_OPTIMIZATION`,
`RATCHET_REPEATS`; bench runs also `RATCHET_RUN` (1-based). **`RATCHET_TRACE`** is set for
`confirm` bench runs only, to the hidden trace (`job.trace`, falling back to `bench.confirm_trace`):

- local runner: a local path is made absolute; URLs are passed through unchanged;
- remote runner: an `s3://` URI is downloaded by the instance (`aws s3 cp`) to
  `<work>/trace/trace.<ext>` (mode 0600, outside the code tree) and `RATCHET_TRACE` is that local path.

Output contract: the last JSON object on stdout; `objective.metric` is read by dotted path; every other
numeric leaf is flattened to a dotted key and stored as `BenchRun.metrics` (arrays and strings are
skipped). Threshold guardrails use the median of each metric over the runs and look the metric up by
exact key, then with a unit suffix (`peak_rss` finds `peak_rss_mb`, as in the demo). Relative bounds
(`< 1.2x`) need the baseline's `metrics` (`medianMetrics(baselineOutcome.runs)` or `metricsFromStats`);
without one they fail, except in the `baseline` phase where they are reported as skipped.

Command guardrails run in the `bench` and `baseline` phases and are skipped in `confirm` (set
`commandGuardrailsInConfirm` to run them there too). They fail fast: the first failure stops the job
before any bench time is spent.

### Remote protocol (what to look at when it breaks)

Steps are separate `runCommand` calls: `ping` (retried while SSM registers), `checkout`, optional
`trace`, `setup`, `guardrail i/n`, `bench run i/n`. Each script is wrapped so it runs under bash even
if the SSM document uses `sh`. Results come back as sentinel lines `@@RATCHET@@ <kind> <json>` on
stdout; SSM returns at most about 24,000 characters per command, so the bench JSON travels as the last
12,000 bytes of stdout, base64 encoded, one command per run. Consequences: the stored log has only
output tails for remote runs (full output stays on the instance in `<work>/logs/` until it is terminated),
and a bench whose final JSON object is larger than 12 kB will not parse. Requirements on the AMI: bash,
git, tar, GNU coreutils (`timeout`, `realpath -m`, `date +%s%N`), GNU grep with `-P`, the `aws` CLI for
`s3://` traces, plus the repo's toolchain (node, pnpm, k6).

## 2. Noise on pinned hardware (PLAN spike B)

Goal: pick `repeats` and `min_gain` from measurement. The rule is
`improvement > max(noise_floor, min_gain * best)` with `noise_floor = 2 * stddev(baseline runs)`
(`noiseFloor()` in core). The floor uses the spread of *single runs*, so more repeats make the median
more reliable but do **not** lower the floor; only less noisy runs do.

Setup: a dedicated `c7i.2xlarge` (Dedicated Host or dedicated tenancy) in one AZ, the bench AMI,
nothing else running, load generator and target on the same instance as in production. Let the
instance sit for a few minutes after boot.

Procedure:

1. **Baseline noise.** 10 consecutive baseline runs of the demo's base commit, with the production
   `bench.run` (60 s + 5 s warmup):
   `RemoteBenchRunner.run({ phase: 'baseline', repeats: 10, ... })`, or on the instance
   `ratchet baseline --repeats 10 --json`. Record the 10 samples of `http_req_duration.p95`, `p50`, `rps`,
   `peak_rss_mb`; compute median, stddev and **CV = stddev / mean** (`summarize()` in core).
2. **Read the CV** (reference: the demo README measured about 11% on a shared 4-vCPU container, p95 is
   dominated by queueing behind occasional slow requests):

   | CV of p95 | What it means | Action |
   |---|---|---|
   | <= 1.5% | `2 * CV <= 3%`: `min_gain: 0.03` is the binding limit | `repeats: 3` to `5` |
   | 1.5% to 4% | the noise floor (`2 * CV`, 3% to 8%) is the binding limit; smaller gains cannot be seen | `repeats: 5`; set `min_gain` to about `2 * CV` so the config tells the truth; or reduce noise first |
   | > 4% | wins below 8% are invisible | reduce noise (below) and re-measure before using this bench |

   Target for v0: CV <= 2% (visible gains from about 4%).
3. **If it is too noisy**, change one thing at a time and repeat step 1: longer duration (120 s), longer
   warmup, a lower arrival rate (stay below saturation so latency is not queueing), pin the server and
   the load generator to disjoint cores (`taskset -c`), stop everything else (cron, SSM agent updates,
   `unattended-upgrades`), compare p50 against p95 (p99 is noisier still), confirm the instance is not
   throttled (`c7i` has no burst credits; check steal time in `mpstat`).
4. **A/A check.** Run 20 *distinct jobs* for the same unchanged sha through the full bench phase
   (`evaluateCandidate` with `currentBest` = the baseline median and `noiseFloor` from step 1). Expect 0
   `pending_confirm` / `verified`. Any hit means the floor is too low for the real variance.
5. **A/B power check.** Apply known changes (one planted problem from the demo's `SOLUTIONS.md` removed;
   a synthetic 3%, 5%, 10% change such as a calibrated `setImmediate` busy-wait) and see which are
   detected. `min_gain` should be about the smallest change that is detected in at least 9 of 10
   attempts.
6. **Write it down**: copy `repeats`, `min_gain`, the 10 samples and the CV into the repo's
   `ratchet.yaml` comments and the ledger baseline. Re-measure after any AMI, kernel or instance-type
   change.

Pass: CV recorded, A/A produces no false win in 20 jobs, the chosen `min_gain` is detected in A/B.

## 3. Confirm run and the sealed bucket

Flow for a candidate that wins the bench phase (`evaluateCandidate` returns `needsConfirm: true`):

1. Engine submits a `phase: 'confirm'` job (`trace` = the sealed object, e.g. `s3://<sealed>/demo-search-trace.json`,
   or omit it to use `bench.confirm_trace`). A new instance is launched for it, so the confirm is on
   fresh hardware by construction.
2. The instance downloads the trace with the bench-runner instance profile (`ReadSealedTraces` in
   `infra/aws/policies/bench_runner_policy.json.tftpl`; agents are explicitly denied on the bucket), then runs
   setup and `bench.run` with `RATCHET_TRACE` pointing at the file. Threshold guardrails (error rate,
   peak RSS against the baseline) are evaluated on the confirm runs.
3. The engine calls `evaluateCandidate({ bench, confirm, confirmBest, ... })`. `confirmBest` must be
   the current best's value **on the hidden trace**; the engine gets it by running the baseline (and
   each new best) once in the confirm phase. Without it core falls back to `currentBest`, which is only
   right if bench and trace share a scale.
4. The instance is terminated; the trace file never leaves it. Only numbers are returned.

Checks:

- [ ] Trace generated with a seed the agents never saw (`bench/traces/generate.ts --seed <hidden>`),
      uploaded to the sealed bucket by an admin identity; agents' roles get `AccessDenied` on it (this is
      also `aws.md` territory; repeat it from a runner here).
- [ ] A confirm job with `s3://` trace: the instance log shows the `trace` step with `bytes` equal to the
      object size; bench output shows the workload from the trace (request count differs from the bench phase).
- [ ] A bench-phase job never has `RATCHET_TRACE` (bench output / `env` dump in a test bench).
- [ ] `confirmBest` flow: baseline measured on the trace once, a candidate that beats the bench mix but
      regresses on the trace ends `failed_confirm`.

### The trust problem you must close before this is real

The candidate's server and the load generator run on the **same instance**, in the same trust domain,
and that instance's profile can read the sealed bucket. Candidate code can therefore try to: read
`RATCHET_TRACE` (environment inherited by the server the bench script spawns; the file is readable by the
same user), fetch instance-profile credentials from IMDS and read the sealed bucket directly, or send what
it finds out over any allowed egress. The demo's `bench/local.ts` spawns the server with the full
environment; a real bench must not. Required before trusting a confirm result against a hostile agent:

1. **Egress lockdown** (below).
2. **Split identities by phase.** Only `confirm` instances need the sealed bucket. Give `bench` and
   `baseline` instances a profile without `ReadSealedTraces` (`BenchLaunchSpec.purpose` carries the
   phase, so the AWS `CloudCompute` can pick the profile), so a candidate running in the bench phase has
   nothing to steal.
3. **IMDS**: require IMDSv2 with hop limit 1 and block 169.254.169.254 for the candidate's user (or run
   the candidate server in a container/network namespace without a route to it).
4. **Separate users**: run the candidate server as an unprivileged user; the load generator (which needs the
   trace) as another; trace file 0600 owned by the load generator. Scrub `RATCHET_TRACE` and all
   `AWS_*` from the server's environment in the bench script (it is in `bench.ref`, so trusted).
5. The instance lives only for one job; do not reuse it across candidates.

The runner implements 5 and the 0600 trace file only. The rest is image/infra work, listed here so the
real test can fail loudly if it is missing:

- [ ] Exfiltration drill: a candidate whose server tries (a) `curl https://example.com`, (b) `aws s3 ls`
      / `aws s3 cp s3://<sealed>/...` using IMDS credentials, (c) reading `/proc/<loadgen>/environ`
      and the trace file, (d) `curl` to a webhook with the trace. In `bench` phase all must fail; in
      `confirm` phase (a), (b), (d) must fail and (c) must fail once users are split.

## 4. Egress lockdown

`infra/aws/network.tf`: the bench security group allows 443 to `var.bench_egress_cidrs`, default
`0.0.0.0/0`, so today candidate code can reach any HTTPS endpoint on the internet from a box that has
sealed-bucket credentials. Needed for the pipeline: SSM (`ssm`, `ssmmessages`, `ec2messages`), S3 (trace,
results), and the candidate/bench fetch. Lock down by:

1. Interface VPC endpoints for SSM, SSM messages and EC2 messages; an S3 gateway endpoint **with an
   endpoint policy** that allows only the artifacts and sealed buckets (an unrestricted S3 endpoint
   lets the candidate write to its own bucket).
2. Remove GitHub from the allowed set: have the controller push the candidate and `bench.ref` to the
   artifacts bucket (e.g. `git bundle`) and let the instance fetch from S3 via the endpoint. Until then, if
   GitHub stays reachable, it is an exfiltration channel too (push to any public repo needs credentials the
   instance does not have, but `curl` to a gist-like service does not).
3. `bench_egress_cidrs` narrowed to the endpoint/prefix-list CIDRs; DNS to the VPC resolver only
   (DNS tunnelling is still possible; Route 53 Resolver DNS Firewall closes it).
4. Setup needs packages (`pnpm install`): either bake `node_modules`/store into the AMI and run
   `pnpm install --offline --frozen-lockfile`, or allow a package mirror inside the VPC. Do **not**
   reopen the internet for setup; if you must, run setup in a separate, earlier instance and ship the
   result (not done today).

Pass: from a bench instance, `curl -m 5 https://example.com` and `https://github.com` fail, SSM and the
allowed S3 prefixes work, a full bench + confirm job succeeds.

## 5. Live smoke checklist for `RemoteBenchRunner`

Run against a real instance with a throwaway repo first, then `examples/demo-search`.

- [ ] `launchBenchInstance` -> `ping` succeeds within `readyAttempts * readyDelayMs`; tune the defaults
      (30 x 10 s) to the real SSM registration time.
- [ ] Checkout of a private repo works: the instance needs git credentials (instance profile + credential
      helper reading SSM `/ratchet/bench/*`, or a short-lived token in `repoUrl`). A token in `repoUrl`
      is visible in SSM command history; prefer the credential helper.
- [ ] `git fetch <url> <full sha>` of a commit that is not a branch tip works (GitHub allows it; the
      script falls back to fetching all branches and tags otherwise).
- [ ] `grep -P`, `realpath -m`, `timeout -k`, `date +%s%N` exist on the AMI.
- [ ] Output limit: a bench that prints 5 MB of logs still parses (only the stdout tail travels);
      `runCommand` does not throw on large output; stored log shows the tails.
- [ ] Timeouts: `timeoutSeconds` (step timeout + 120 s) is honoured by the real `runCommand`; a hung
      bench is killed by the in-script `timeout -k 10` and reported as `timed out after ...ms`.
- [ ] Abort mid-run terminates the instance within seconds and `listTagged` shows no leftover.
- [ ] A failing `terminate` raises `onWarning`, and the reaper picks the instance up.
- [ ] Tamper drills on the instance: a candidate whose `pnpm build` rewrites `bench/*` ends `blocked`
      with `protected files intact`; a candidate that replaces `bench` by a symlink to `/` does not
      change files outside the checkout; the score is the one from `bench.ref`.
- [ ] Cost: each job pays instance start-up (about 1 to 2 minutes with SSM registration) plus setup;
      measure it, decide whether to keep a warm pool (not implemented, and a warm instance reused across
      candidates weakens the isolation above).

## 6. What is mocked in the automated tests

| Real thing | Test stand-in | What that does *not* prove |
|---|---|---|
| EC2 bench instance + SSM `RunShellScript` | `LocalFakeCompute`: runs each generated script with local `bash`, truncates stdout to 24,000 characters, an `aws` shim copies `s3://b/k` from a directory | SSM latency and registration, the document's shell, real command timeouts and polling, IAM, IMDS, security groups, AMI contents, that the instance's `git` can reach the repo; `terminate` there kills the process tree, which a real termination does by destroying the machine |
| GitHub / git remote | `LocalGitHost` (`git archive` of a local repo) | auth, rate limits, fetch-by-sha policy of the host |
| Hidden trace in the sealed bucket | a local file / shimmed `s3://` path | bucket policy, KMS, that agents cannot read it |
| k6 | not used; the demo test runs `node dist/bench/local.js --duration 3 --warmup 1` | k6 JSON output (`RESULT_FILE`) and the k6 script's own contract |
| Noise | tests assert logic, not variance | everything in section 2 |

Automated coverage that does exist: both runners against the same scenarios (guardrail failure before
the bench, protected overlay incl. symlink attack and tamper detection, repeats, metric flattening,
threshold guardrails vs baseline, confirm trace env, abort, timeouts, setup, partial failures), the
queue (serialization, round-robin fairness, cancellation, events), `evaluateCandidate`, and a real run
on a copy of `examples/demo-search` (about 20 s).

Manual command for the demo integration (what `src/integration.test.ts` automates; skip it with
`RATCHET_SKIP_DEMO=1`):

```sh
cp -r examples/demo-search /tmp/demo && cd /tmp/demo && rm -rf dist node_modules
git init -q && git add -A && git commit -qm base && git branch ratchet/bench
tsc -p tsconfig.build.json   # needs ../../tsconfig.base.json next to it, see the test
node dist/bench/local.js --duration 3 --warmup 1 --port 4123
```
