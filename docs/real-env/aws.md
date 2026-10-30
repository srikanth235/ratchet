# Real-environment testing: AWS (`@ratchet/aws`)

Everything in `packages/aws` was built against the AWS SDK for JavaScript v3
and tested with `aws-sdk-client-mock` and a fake process runner. **No call has
touched a real AWS account.** This file is the procedure for doing that, one
implementation at a time, in a **dedicated sandbox account** (never a shared
or production account).

## What the unit tests do and do not prove

| Implementation | Unit tests prove | Only a real account proves |
|---|---|---|
| `TofuSandboxProvider` | CLI sequence, args, vars file, plan/output parsing, destroy guard, per-sandbox locking | `-state`/`TF_DATA_DIR` behave as assumed with a saved plan, the module applies cleanly, outputs shape |
| `AwsCloudCompute` | Exact request params: role path, boundary, tags, IMDSv2, terminate-on-shutdown, profile (bench profile for `baseline`/`bench`, confirm profile only for `confirm`, fail closed otherwise), subnet (bench subnet for bench), deletion order, SSM paths; that `*`, `?`, `/`, `..` are rejected as ids at every entry point | IAM eventual-consistency retry is enough, the boundary really permits/denies as intended, SSM agent reachability, that the iptables/user bootstrap works on the AMI |
| `AwsPriceBook` | Filters sent, price parsing, cache | Filters match real Pricing API attribute values |
| `S3ArtifactStore` | Request params, paging, NoSuchKey | Bucket policy / SSE / the agent prefix restriction |
| `CostMeter`, `BudgetGuard`, `Reaper` | Math, rules, idempotency (against `MockCloudCompute`) | Tag filters return what we expect; `StateTransitionReason` parsing; real instance lifetimes |
| `AwsCostReconciler` (+ `reconcileSpend`) | `GetCostAndUsage` request shape (tag filter, group-by, daily window, paging), tag-to-category mapping, completeness gates, idempotent correction rows (mocked SDK client, in-memory ledger) | Cost Explorer accepts the query, the group keys look as assumed (`ratchet:role$bench`), `Estimated` behaves as assumed, the tags are activated and show up, real deviation from the estimate |

## 0. Prerequisites

- A dedicated AWS account with an SSO profile; `export AWS_PROFILE=ratchet-sandbox AWS_REGION=us-east-1`.
- A pinned `tofu` binary (`tofu version`) and this repo built: `pnpm install && pnpm build`.
- A runner AMI and a bench AMI, built from `infra/images` (see "Build images" below). Until they exist, use any
  SSM-enabled Amazon Linux AMI: instances will launch, tag, accrue cost and be SSM-reachable, which is all
  steps 2-6 need. The bench AMI must also carry `iptables`, `setpriv`, `pkill`, `sudo`, `shred`, GNU
  `find`/`grep -P`/`sha256sum`/`cp` and a world-readable toolchain (see `bench.md`); `infra/images` installs them.
  The runner AMI is first needed for `agents.md`, the bench AMI for `bench.md`.
- Billing alarms on the account (the kill switch is a backstop, not a substitute).

Run the snippets below from a scratch script (`node --input-type=module -e '...'` or a
`.mjs` file) importing from `packages/aws/dist/index.js`.

## Build images

Live mode launches two machine images that nothing else creates; build them once per region, and again when a
pinned tool or `packages/agents` changes. The Packer definitions and scripts are in `infra/images` (procedure,
reproducibility, and the EC2 Image Builder alternative: `infra/images/README.md`).

```sh
export AWS_PROFILE=ratchet-sandbox AWS_REGION=us-east-1     # the sandbox account; the builder needs outbound internet
infra/images/build.sh both          # packs the runner tarballs, runs packer for runner and bench
# runnerImageRef: ami-0123456789abcdef0
# benchImageRef: ami-0fedcba9876543210
```

- **Runner** (`ratchet:image=runner`): Node 22, pnpm, git, `claude` and `codex` (pinned), AWS CLI v2, SSM agent, perf,
  hyperfine, k6 and the `ratchet-runner` bin from this checkout.
- **Bench** (`ratchet:image=bench`): **generic** since the repo's container replaced per-repo toolchains: the Docker engine,
  git, AWS CLI v2, SSM agent, zstd, iptables, the unprivileged `ratchet-bench` and `ratchet-holdout` users and the setuid
  `ratchet-candidate-ctr` launcher. No language toolchain, no agent CLIs, no model keys, no compiler. The repo's toolchain is
  its container (`environment.image`, `bench.md` "The repo's container"). `-var host_toolchain=true` builds the legacy variant with
  node, pnpm, k6 and hyperfine (tagged `ratchet:image=bench-host`, never found by the lookup) for repos without `environment.image`.
  The IMDS block for the users is added by the instance user data (`benchUserData`), not baked into the image.
- **You do not pass the ids.** The app looks the images up by tag (`ratchet:image=runner|bench`, `ratchet:image_version` equal to the
  app version or the app version plus a build suffix; newest wins; own account, the sandbox's region, the architecture of the instance
  types): `findRatchetImages(version, { region })` / `resolveImageRefs` in `packages/aws`, wired into
  `createLiveRuntimeFor(opts, config)` / `resolveLiveImages(opts)` in `packages/runtime/src/live.ts`. `build.sh` stamps the app version
  (`packages/desktop` version, or `RATCHET_APP_VERSION`) into the tag. A missing image is an `ImageLookupError` that says which image to
  build. The explicit override stays for development only (Settings > Advanced):
  `createLiveRuntime({ ..., runnerImageRef: 'ami-...', benchImageRef: 'ami-...' })`; it is used verbatim as `ImageId`, so it must be
  the plain `ami-...` id of the sandbox's region and account (the AMIs are encrypted by default).
- **Region per optimization.** `environment.region` in `ratchet.yaml` (read defensively; a value that is not a region name is an error)
  picks the region of that optimization's runtime and image lookup (`regionForOptimization`, `createLiveRuntimeFor`); the setting
  and `us-east-1` are the fallbacks. AMIs and the sandbox are regional: build / apply in every region you use.
- **Custom sandbox module (advanced, off by default).** `infra: <dir>` in `ratchet.yaml` names a module in the repository. It is
  read from the protected ref only (`materializeInfraOverride`: no symlinks, `.tf` files present, and no `local-exec` / `remote-exec` /
  `external` data source, because a module runs with your credentials), and after every apply its outputs are validated against
  `infra/contract.md` (`checkSandboxContract`, enabled with `TofuSandboxProvider({ validateContract: true })`, `LiveOptions.infraOverride`)
  with a message that lists every missing or malformed output and every broken separation (buckets, identities, subnets, groups).
  TODO: the engine's sandbox lifecycle does not yet call `readInfraOverride` / `materializeInfraOverride` and pass the resulting
  `moduleDir`; the functions and the post-apply validation are in place and tested.
- **Registries for the image build.** The container image job runs in the agent subnet; the registries its Dockerfile pulls from are
  not in the default DNS / proxy allow-list. Add them for your repo (`egress_proxy_extra_domains` with the proxy,
  `dns_firewall_extra_domains` without): typically `registry-1.docker.io`, `auth.docker.io`, `production.cloudflare.docker.com`,
  `ghcr.io`, `public.ecr.aws`, plus OS package mirrors used by `RUN` steps. That is an operator decision (`environment.network` is the
  per-repo expression of it); nothing about the bench subnet changes.
- **Check:** `aws ec2 describe-images --image-ids <ami> --query 'Images[].Tags'` shows `ratchet:image`,
  `ratchet:image_version` and `ratchet:source_commit`; `bench.md` section 5 exercises the bench AMI, and the first
  `launchRunner` in `agents.md` the runner AMI.
- **Without AWS:** `pnpm --filter @ratchet/infra-tests test` checks the scripts' invariants (pins, no agent CLIs on the
  bench, the `ratchet-bench` user, the AMI tags) and `packer validate` the templates.

## 1. `TofuSandboxProvider`

```js
import { TofuSandboxProvider, listAgentRoles } from '@ratchet/aws';
import { IAMClient } from '@aws-sdk/client-iam';
const iam = new IAMClient({ region: 'us-east-1' });
const input = { sandboxId: 'rt-test', region: 'us-east-1', allowedInstanceTypes: ['c7i.large', 'c7i.2xlarge'],
                maxInstancesPerAgent: 4, maxInstanceLifetimeHours: 2 };
const sandbox = new TofuSandboxProvider({
  tofuBinary: '/path/to/tofu', moduleDir: `${process.cwd()}/infra/aws`, stateRoot: '/tmp/ratchet-sandboxes',
  env: { AWS_PROFILE: 'ratchet-sandbox' },
  listRuntimeAgentRoles: () => listAgentRoles(iam),
});
console.log(await sandbox.plan(input));   // then apply, outputs, destroy
```

| Step | Command | Expected |
|---|---|---|
| plan | `sandbox.plan(input)` | every change is `create`: buckets and their hardening, the boundary, **three** runner identities (`bench_runner`, `confirm_runner`, `deps_runner`: role, profile, policy each), the VPC, subnets, route tables, the S3 gateway endpoint, 4 interface endpoints (`ssm`, `ssmmessages`, `ec2messages`, `logs`), the endpoint/agent-runner/bench security groups, the DNS Firewall (2 domain lists, 1 rule group, 2 rules, 1 association, 1 config; skipped with `enable_dns_firewall=false`) and the second lifecycle rule on the artifacts bucket. Roughly 61 resources; no proxy resources unless `enable_egress_proxy`. (`SANDBOX_RESOURCES` in `sandbox/mock.ts` lists the headline ones.) |
| state location | `ls /tmp/ratchet-sandboxes/rt-test` | `vars.tfvars.json`, `plan.tfplan`, `.terraform-data/`; and **no** `terraform.tfstate` or `.terraform*` in `infra/aws/` (`git status infra/` clean) |
| apply | `sandbox.apply(input)` | resolves with `artifactsBucket`, `sealedBucket`, `agentBoundaryRef` (ARN), `agentRolePath: '/ratchet/agent/'`, network ids incl. `benchSubnetId` (different from `subnetId`) and `egressProxyUrl: null`, `evalIdentity`, `confirmIdentity` **and** `depsIdentity` (three different profiles; `depsIdentity` is optional when parsing the outputs of a sandbox applied before dependency snapshots, which then needs a re-apply), `sealedConfirmPrefix: 'holdout/'`, `limits` `{4, 2}`; `terraform.tfstate` now present under the sandbox dir; `plan.tfplan` removed |
| idempotent | `sandbox.plan(input)` again | every change `no-op` |
| outputs | `sandbox.outputs('rt-test')` (new process) | same values; `region` restored from `vars.tfvars.json`; unknown id -> `null` |
| console check | `aws s3api get-public-access-block --bucket <sealed>` | all four true |
| network | `aws ec2 describe-route-tables --filters Name=vpc-id,Values=<vpc>` | the route table of the bench subnet has only the `local` route and the S3 prefix list; the only `0.0.0.0/0` route (to the internet gateway) is on the public table; `describe-vpc-endpoints` lists one gateway and four interface endpoints, all `available`, private DNS enabled; the default security group has no rules |
| endpoint policy | `aws ec2 describe-vpc-endpoints --filters Name=service-name,Values=com.amazonaws.<region>.s3 --query 'VpcEndpoints[0].PolicyDocument'` | only the two sandbox buckets |
| sealed policy | `aws s3api get-bucket-policy --bucket <sealed>` | statements `DenyAgentRolesByPath`, `DenyAnyPrincipalTaggedAsAgent`, `DenyBenchRunner`, `DenyDepsRunner`, `DenyInsecureTransport` |
| roles | `aws iam simulate-principal-policy --policy-source-arn <bench_runner role> --action-names s3:GetObject --resource-arns arn:aws:s3:::<sealed>/holdout/x` | `implicitDeny` (and explicit deny from the bucket policy); the same for the confirm runner role is `allowed`, and `.../other/x` is `implicitDeny` |
| deps role | the same simulation for `<deps_runner role>`: `s3:PutObject` on `<artifacts>/bench-deps/k/a.tgz` -> `allowed`; `s3:PutObject` on `.../bench-results/x`, `.../artifacts/opt1/a1/x`, `.../bench-inputs/o/j/code.tgz` -> `implicitDeny`; `s3:GetObject` on `.../bench-inputs/o/j/deps-base.tgz` -> `allowed`, on `.../bench-inputs/o/j/code.tgz` -> `implicitDeny`; anything on `<sealed>` -> `explicitDeny`. The bench and confirm roles: `GetObject` on `.../bench-deps/k.json` `allowed`, `PutObject` there `implicitDeny` |

**Unverified assumption to check first:** we pass `-state=<path>` to both `plan` and
`apply <planfile>`. If `tofu apply` rejects `-state` with a saved plan, drop it from
`apply` in `TofuSandboxProvider.apply` (the plan file records the state path). Also
confirm `init -backend=false` with `TF_DATA_DIR` re-initialises cleanly on each call.

**Destroy guard** (do this before step 7's teardown): with a runtime agent role present
(step 2), `sandbox.destroy(input)` must throw `TeardownBlockedError` listing the role and
must not run `tofu destroy` (`aws s3 ls` still shows the buckets).

### 1b. Sandbox lifecycle through the engine (what the desktop app does)

The app never calls the provider directly: `engine.planSandbox / applySandbox / destroySandbox` wrap it in `SandboxLifecycle`
(`packages/core/src/sandbox`) and write the ledger. Run the same thing from a script to check the recorded state:

```js
import { Engine } from '@ratchet/engine';
import { createLiveRuntime } from '@ratchet/runtime';
// ledger = openLedger('/tmp/rt.db'); rt = createLiveRuntime({ ledger, region, sandbox: null, sandboxId: 'rt-test',
//   tofuBinary, moduleDir, stateRoot, awsProfile: 'ratchet-sandbox' })   // no outputs yet: only the sandbox ports work
await rt.engine.planSandbox(input);      // sandboxes row: planned
await rt.engine.applySandbox(input);     // applying -> applied; outputs + account_id stored
await rt.engine.sandboxState('rt-test'); // { status: 'applied', outputs, accountId, optimizations: [] }
await rt.engine.destroySandbox('rt-test');
```

| Check | Expected |
|---|---|
| after `plan` | `select status,provider from sandboxes` -> `planned|aws`; nothing in `<stateRoot>/rt-test` except vars and the plan file |
| during `apply` (second shell) | status `applying`; a second `applySandbox` fails fast with "busy" (and tofu's own per-sandbox lock) |
| after `apply` | status `applied`, `account_id` = the account, `outputs` JSON has `network.benchSubnetId`, `evalIdentity`, `confirmIdentity` (what `createLiveRuntime` feeds to `awsComputeConfigFromOutputs`) |
| kill the process mid-apply, start a new engine | the row is `failed` (message "interrupted"); `applySandbox` again converges to `applied` |
| `apply` with a bad profile / no credentials | `failed`, the error text carries tofu's stderr tail; retry after fixing succeeds |
| `destroy` with a running optimization linked to the sandbox | refused (`sandbox_in_use`), status unchanged, no `tofu destroy` |
| `destroy` with an agent identity left (optimization stopped, no teardown) | refused (`orphans`), status unchanged |
| `destroy` with a tagged instance alive | refused (`orphans`) listing the instance ids; this is the same `listTagged` check `teardown()` ends with |
| `destroy` with a runtime agent role still present but no engine record (created by hand, step 2) | the provider throws `TeardownBlockedError`; status goes back to `applied` (not `failed`) |
| clean `destroy` | `destroying` -> `destroyed`, `outputs` NULL, `destroyed_at` set; `tofu state list` empty; zero-orphan list in §8 is empty |
| `destroy` where `tofu destroy` itself fails | `failed` (real state unknown); retry after fixing the cause |

Named profiles: `AwsCloudCompute`, `S3ArtifactStore` and `AwsPriceBook` take `profile` (the SDK's own profile resolution, so SSO and
assume-role profiles work without touching `process.env`); tofu gets `AWS_PROFILE`. With an SSO profile, run `aws sso login` first
and check that a long apply does not outlive the session. `checkAwsCredentials({ region, profile, expectedAccountId })` is the "Test
connection" for AWS: it must fail for another account id and surface an expired SSO session.

## 2. `AwsCloudCompute`: identity, secrets

```js
import { AwsCloudCompute, awsComputeConfigFromOutputs } from '@ratchet/aws';
const outs = await sandbox.outputs('rt-test');
const cloud = new AwsCloudCompute(awsComputeConfigFromOutputs(outs, 'rt-test'));
console.log(await cloud.preflight('<sandbox account id>'));   // wrong account id must throw
const id = await cloud.createAgentIdentity('opt1', 'a1');
```

| Check | Command | Expected |
|---|---|---|
| role | `aws iam get-role --role-name ratchet-agent-opt1-a1` | `Path=/ratchet/agent/`, `PermissionsBoundary.PermissionsBoundaryArn == agent_boundary_ref`, tags `ratchet:opt=opt1`, `ratchet:agent=a1`, `ratchet:sandbox=rt-test` |
| policy | `aws iam get-role-policy --role-name ratchet-agent-opt1-a1 --policy-name ratchet-agent-permissions` | S3 only `artifacts/opt1/a1/*`; SSM only `/ratchet/opt1/a1/*`; no `iam:*` |
| profile | `aws iam get-instance-profile --instance-profile-name ratchet-agent-opt1-a1` | contains the role |
| idempotent | call `createAgentIdentity('opt1','a1')` again | same refs, no error |
| bad ids | `createAgentIdentity('Opt/1','a1')` | throws before any AWS call (CloudTrail shows nothing) |
| secret | `cloud.putSecret('opt1','a1','TEST_KEY','x')` | `aws ssm get-parameter --name /ratchet/opt1/a1/TEST_KEY --with-decryption` returns `x`, type `SecureString`; `aws ssm list-tags-for-resource --resource-type Parameter --resource-id /ratchet/opt1/a1/TEST_KEY` shows opt/agent tags |

Boundary behaviour (this is what the mocks cannot tell you): use
`aws iam simulate-principal-policy --policy-source-arn <role arn> --action-names s3:GetObject --resource-arns arn:aws:s3:::<sealed>/x`
-> `explicitDeny`; same action on `arn:aws:s3:::<artifacts>/artifacts/opt1/a1/x` -> `allowed`;
on `.../artifacts/opt1/OTHER/x` -> `implicitDeny`; `iam:CreateRole`, `sts:AssumeRole` -> `explicitDeny`.
(Simulator caveat: principal-tag variables resolve only if the tags are on the role, which they are.)

## 3. `AwsCloudCompute`: launch, SSM, terminate, list

```js
const runner = await cloud.launchRunner({ optimizationId: 'opt1', agentId: 'a1', kind: 'claude-code',
  instanceType: 'c7i.large', imageRef: 'ami-...', identity: id, secrets: [{ name: 'TEST_KEY', ref: '/ratchet/opt1/a1/TEST_KEY' }],
  repoUrl: 'https://github.com/<you>/<repo>.git', branch: 'ratchet/opt1/a1' });
```

| Check | Command | Expected |
|---|---|---|
| launch | `aws ec2 describe-instances --instance-ids <id>` | `MetadataOptions.HttpTokens=required`, `InstanceInitiatedShutdownBehavior` = `terminate` (`aws ec2 describe-instance-attribute --attribute instanceInitiatedShutdownBehavior`), subnet = `network_ref.subnet_id` (agent subnet), SG = agent_runner SG, profile = agent profile, tags `ratchet:opt/agent/role=agent_runner/sandbox`, volume tagged too |
| IAM propagation | launch immediately after `createAgentIdentity` | succeeds, possibly after retries (log the number; if >6 retries are needed, raise `iamPropagationRetries`) |
| user-data | `aws ec2 describe-instance-attribute --attribute userData` (base64 decode) | contains refs (`RATCHET_SECRET_REFS`), **no secret values**, `shutdown -h +120`; with the proxy enabled also `HTTPS_PROXY`, `NO_PROXY` and `/etc/profile.d/ratchet-proxy.sh` |
| list | `cloud.listTagged('opt1')` | the runner with `role: 'agent_runner'`, `state` pending->running |
| SSM | `cloud.runCommand(runner.id, { script: 'echo $X; id -un', env: { X: "a'b" } })` | `{exitCode:0, stdout:"a'b\n..."}`; `exit 7` -> `exitCode 7`; a `sleep` beyond `timeoutSeconds` -> exit 124. Note: SSM truncates stdout at 24000 chars |
| bench | `cloud.launchBenchInstance({ optimizationId:'opt1', instanceType:'c7i.2xlarge', imageRef:'ami-...', purpose:'baseline' })` | eval (bench-runner) instance profile, **bench subnet** (`network_ref.bench_subnet_id`), bench SG, no public IP, `HttpPutResponseHopLimit=1`, no `ratchet:agent` tag, role=`bench`, `ratchet:purpose=baseline` |
| deps | `cloud.launchBenchInstance({ optimizationId:'opt1', instanceType:'c7i.2xlarge', imageRef:'ami-...', purpose:'deps' })` | the `...-deps-runner` instance profile, the **agent subnet** (`network_ref.subnet_id`) with the **agent_runner SG** (egress; through the proxy when enabled), IMDSv2 required, hop limit 1, `InstanceInitiatedShutdownBehavior=terminate`, tags `ratchet:opt`, `ratchet:role=bench`, `ratchet:purpose=deps`, no `ratchet:agent`; user-data creates `ratchet-deps`, blocks IMDS for it, has `shutdown -h +60`, the proxy variables when the proxy is on, and none of the bench machinery (no launcher, no sudoers, no `ratchet-bench`). With `depsInstanceProfile` unset (sandbox not re-applied) the call throws and launches nothing; it never falls back to the bench subnet or profile |
| bench (confirm) | the same with `purpose:'confirm'` | the `...-confirm-runner` instance profile; every other attribute identical. With `confirmInstanceProfile` unset the call throws and launches nothing |
| bench user-data | decode the bench instance's user-data | creates `ratchet-bench` **and** `ratchet-holdout`, installs the `iptables -I OUTPUT 1 -m owner --uid-owner <user> -d 169.254.169.254 -j REJECT` rule for both, remounts `/proc` with `hidepid=2`, installs `/usr/local/sbin/ratchet-candidate-exec`, `/usr/local/bin/ratchet-candidate` and `/etc/sudoers.d/ratchet-holdout` (checked with `visudo -cf`), all before touching `/run/ratchet/user-data.done`. On the instance: `sudo -u ratchet-bench curl -m 3 http://169.254.169.254/latest/meta-data/` and the same as `ratchet-holdout` fail, `curl` as root works; `sudo -l -U ratchet-holdout` lists exactly `(ratchet-bench) NOPASSWD: /usr/local/sbin/ratchet-candidate-exec`; `sudo -l -U ratchet-bench` says it is not in the sudoers file; `sudo -u ratchet-holdout /usr/local/bin/ratchet-candidate id -un` prints `ratchet-bench` (with `RATCHET_CANDIDATE_DIR/HOME/TMP/PATH` exported) |
| boundary network | from the runner: `run-instances` into the bench subnet, into the proxy subnet, and with `--security-group-ids <bench sg>` (each separately) | all explicitly denied (`DenyRunInstancesInInternalSecurityGroups`, subnet not in `AllowRunInstancesAgentSubnetOnly`); into the agent subnet with the agent_runner SG it succeeds |
| experiment role | from the runner (`aws ec2 run-instances ... --tag-specifications` with only opt+agent tags, IMDSv2, no profile) | succeeds under the agent role; `listTagged` classifies it `experiment`. The same call without tags, with other tags, with `HttpTokens=optional`, or with a non-allow-listed type must be denied |
| terminate | `cloud.terminate([ids])`, twice | no error the second time; `describe-instances` shows `shutting-down` -> `terminated` |

## 4. Kill switch

1. `cloud.revokeAgentIdentity(id)` with the runner up.
2. Within seconds, on the runner: `aws s3 ls s3://<artifacts>/artifacts/opt1/a1/` and
   `aws ec2 describe-instances` must both fail `AccessDenied` using the *already issued* instance
   credentials (deny-all applies to existing sessions). **Measure and record the delay**; if
   not immediate, add a `aws:TokenIssueTime` revoke-sessions deny to `revokeAgentIdentity`.
3. `aws iam list-role-policies --role-name ratchet-agent-opt1-a1` lists `ratchet-kill-switch`.

## 5. `AwsPriceBook`, `StaticPriceBook`

```js
const book = new AwsPriceBook({ region: 'us-east-1', fallback: new StaticPriceBook() });
await book.hourlyUsd('c7i.2xlarge');   // ~0.357 in us-east-1
```

Expected: matches the EC2 pricing page / `aws pricing get-products --service-code AmazonEC2 --region us-east-1 --filters ...`
(one `GetProducts` call, then cached). Compare the `StaticPriceBook` table against it and update
`STATIC_HOURLY_USD` if drifted (it is approximate us-east-1 Linux on-demand). Try
`region: 'eu-west-1'` to confirm `regionCode` filtering; `instanceType: 'nope.large'` must throw
(or fall back only if you set `fallback`).

## 6. `S3ArtifactStore`

```js
const store = new S3ArtifactStore({ bucket: outs.artifactsBucket, region: 'us-east-1' });
await store.put('artifacts/opt1/a1/events.jsonl', '{"a":1}\n');
```

Expected: `get` returns the text, `get` of a missing key returns `null`, `list('artifacts/opt1/')`
returns the key; object shows `ServerSideEncryption: AES256`. From the **agent role** (on the
runner): put/get under `artifacts/opt1/a1/` works; any other prefix, bucket-root `list`, and every
operation on the sealed bucket fail. `FsArtifactStore` needs no account.

## 7. `CostMeter`, `BudgetGuard`

```js
const meter = new CostMeter({ compute: cloud, prices: book });
setInterval(async () => console.log(await meter.collect('opt1')), 60_000);
```

Leave one `c7i.large` runner for ~30 min. Expected: `collect()` deltas sum to
`0.5 h x price` (+/- one polling interval); `snapshot().instances[].billedSeconds` tracks
`now - LaunchTime`. After termination, the deltas stop; `snapshot` for a terminated instance uses the
parsed `StateTransitionReason` timestamp (check `listTagged` returns `terminatedAt` for a
just-terminated instance; if it does not, the meter falls back to first-observation time).
Reconcile against Cost Explorer filtered by tag `ratchet:opt` after ~24 h and record the
deviation (expected: small, EBS/data transfer not metered; the ledger categories `storage_egress`
and `tokens` are not produced by this meter). `BudgetGuard.update(total)`: warns once at 80 %,
trips once at 100 % (wire `onTrip` to revoke + reaper; see 8).

Known gap: terminated instances vanish from `DescribeInstances` after about an hour. Run
`collect()` at least once a minute so the final slice is captured.

### 7b. Cost Explorer reconcile (`AwsCostReconciler`)

The cost meter is an estimate (tagged instance lifetimes x on-demand price). The reconcile step compares it
with what AWS billed and appends correction rows to the ledger (`spend_events`: `estimated = false`,
`source = 'cost-explorer'`, one row per category with a non-zero difference, positive or negative; the
estimate rows are never touched). Entry points, all running the same `reconcileSpend` (core):

- teardown: `Engine.teardown(id)` calls it best effort and records the outcome in the teardown report
  (`costReconcile`). It never fails the teardown. Right after teardown the answer is almost always `pending`.
- CLI: `ratchet cost reconcile <optimizationId> --db <ratchet.db> [--json] [--lag-hours 24] [--accept-estimated]`
  (`--db` falls back to `$RATCHET_DB`).
- Engine API: `Engine.reconcileCosts(id)` / `EngineApi.reconcileCosts?` (the desktop can call it later), report
  readable with `Engine.costReconcileReport(id)`.

**Required IAM permission** on the identity that runs the app/CLI (SSO role, not the agent roles; the agent
boundary explicitly denies billing/cost):

```json
{ "Effect": "Allow", "Action": "ce:GetCostAndUsage", "Resource": "*" }
```

Cost Explorer has no resource-level permissions and one global endpoint (`us-east-1`; the client is created
for that region regardless of the sandbox region). The Cost Explorer API costs $0.01 per request; one
reconcile makes one request per page (usually 1). `infra/aws/*.tf` does not grant this; add it to whichever
policy the operator's role uses.

**Cost-allocation tags must be activated, once, by hand.** Tags only appear in Cost Explorer after they are
activated as *user-defined cost allocation tags* in the **management (payer) account** of the organization
(for a standalone account, that account): Billing and Cost Management > Cost allocation tags > select
`ratchet:opt`, `ratchet:agent`, `ratchet:role`, `ratchet:sandbox` > Activate (CLI:
`aws ce update-cost-allocation-tags-status --cost-allocation-tags-status TagKey=ratchet:opt,Status=Active ...`).
A tag key is listed only after a resource carrying it has existed (launch one instance first; up to 24 h),
and activation takes up to another 24 h to take effect. Activation is not retroactive: usage from before the
activation is not attributed by tag. Until then the query returns no cost, which the reconcile reports as
`pending` (never as a $0 bill).

What is queried: `GetCostAndUsage`, `DAILY`, metric `UnblendedCost`, filter `ratchet:opt = <id>`, group by tags
`ratchet:role` and `ratchet:agent`, window = optimization start day to the day it ended. Groups map to ledger
categories: `role=agent_runner` -> `agent_runner`; `role=bench` -> `bench_runner`; `role=experiment`, or no role
but an agent tag (agent-launched experiments carry only `ratchet:opt` + `ratchet:agent`) -> `experiment`;
anything else tagged only with `ratchet:opt` -> `storage_egress`. EBS volumes carry their instance's tags, so
they land with it. `tokens` are never reconciled (AWS does not bill them). Not captured: cost of resources
without a `ratchet:opt` tag (the long-lived sandbox: VPC endpoints, buckets), credits and refunds (unblended
cost is before credits).

Writes happen only when the data can be final; otherwise the status is `pending` and the ledger is untouched:
the optimization has not ended; less than 24 h (`--lag-hours`, `EngineSettings.costLagMs`) since it ended;
any returned day is flagged `Estimated` (override with `--accept-estimated`); or Cost Explorer returned no
cost while the ledger has an estimate (tags not activated, or data not there yet). Idempotence: each
correction is `billed - everything the ledger already holds for that category`, so re-running with the same
billing data writes nothing, and later, higher data writes only the remainder.

Real-account check (after the 24 h wait of step 7):

1. Activate the four tags (above), run a short real optimization (one `c7i.large` runner, one bench run), tear it down.
2. Immediately: `ratchet cost reconcile <opt> --db ...` -> `pending` (still inside the 24 h gate), no rows.
3. After 24-48 h: run it again. Expected: `reconciled`, one correction row per metered category, billed within a few percent of the
   estimate (EBS and per-second rounding explain small differences). `select * from spend_events where source='cost-explorer'`
   shows `estimated = 0`. Run it again: `unchanged`.
4. **Verify the assumptions the mocks cannot:** the request is accepted (a `ValidationException` means the tag-group or date
   assumption is wrong); `Groups[].Keys` look like `ratchet:role$bench` / `ratchet:agent$` (the parser relies on the
   `<key>$<value>` form); `Estimated` is `false` for days older than ~1 day (if it is `true` for the whole current month,
   every reconcile stays `pending`: use `--accept-estimated` and note that finalised numbers can still change until the month closes);
   `ce:GetCostAndUsage` is the only permission needed; an identity without it gets `AccessDeniedException` and the CLI prints this doc's path.
5. Negative case: with tags **not** activated the result is `pending` ("cost-allocation tags"), not a correction to $0.

## 8. `Reaper` and zero-orphan teardown

```js
const reaper = new Reaper({ compute: cloud, maxInstanceLifetimeHours: 2, maxInstancesPerAgent: 4,
  isOptimizationEnded: (o) => ended.has(o), isAgentStopped: (o, a) => stopped.has(`${o}/${a}`) });
```

| Scenario | Setup | Expected |
|---|---|---|
| lifetime | launch an instance; temporarily construct the reaper with `maxInstanceLifetimeHours: 0.01` | terminated, `reason: 'max-lifetime'`; second `reap()` -> `terminated: []` |
| stopped agent | add the agent to `stopped` | its runner + experiments terminated (`agent-stopped`), other agents untouched |
| ended optimization | add opt to `ended` | everything tagged with it, including bench, terminated |
| quota | from the runner launch 6 experiments | newest 2 terminated (`over-quota`), `overQuotaAgents` lists the agent |
| stopped instance | `aws ec2 stop-instances` on a tagged instance | terminated (`stopped-instance`) |
| shutdown behaviour | `sudo shutdown -h now` on the runner | instance goes straight to `terminated` (not `stopped`) |

**Zero-orphan teardown** (run after every real test, and make it the exit criterion):

1. `cloud.revokeAgentIdentity(id)` (kill switch).
2. `reaper.reap()` with the optimization marked ended; then `cloud.terminate` anything left.
3. `cloud.deleteSecrets('opt1','a1')`.
4. `cloud.deleteAgentIdentity(id)` (twice; second must be a no-op).
5. `sandbox.destroy(input)`.

Verify nothing remains (all must be empty):

```sh
aws ec2 describe-instances --filters Name=tag-key,Values=ratchet:opt Name=instance-state-name,Values=pending,running,stopping,stopped \
  --query 'Reservations[].Instances[].InstanceId'
aws ec2 describe-volumes --filters Name=tag-key,Values=ratchet:opt --query 'Volumes[].VolumeId'
aws ssm get-parameters-by-path --path /ratchet --recursive --query 'Parameters[].Name'
aws iam list-roles --path-prefix /ratchet/ --query 'Roles[].RoleName'
aws iam list-instance-profiles --path-prefix /ratchet/ --query 'InstanceProfiles[].InstanceProfileName'
aws iam list-policies --scope Local --path-prefix /ratchet/ --query 'Policies[].PolicyName'
aws s3api list-buckets --query "Buckets[?starts_with(Name,'ratchet-rt-test')].Name"
```

Also confirm Cost Explorer shows no spend growth the next day.

## 9. Egress proxy (only if `enable_egress_proxy = true`)

Default is off; do this section only when you turn it on (`-var enable_egress_proxy=true`). Cost is about
$7.5/month on top of the always-on network (about $29/month for the four interface endpoints); see
`infra/README.md`. With the proxy the agent subnet has **no route to the internet**: the only ways out are the
proxy on 3128, the interface endpoints and the S3 prefix list.

| Check | Command (on an agent runner, via SSM) | Expected |
|---|---|---|
| plan/apply | `tofu plan -var enable_egress_proxy=true` | adds `aws_subnet.proxy`, its security group, `aws_instance.egress_proxy`; the agent subnet's route-table association changes to the private table; `network_ref.egress_proxy_url` is `http://10.42.3.10:3128` |
| proxy up | `systemctl status squid` on the proxy (no SSM profile: use EC2 serial console or a temporary key) / `curl -x $HTTPS_PROXY -I https://api.anthropic.com` | squid listening; `HTTP/1.1 200 Connection established` then the upstream answer |
| allow-list | `curl -m 8 -x $HTTPS_PROXY https://example.com` and `https://api.github.com` | `example.com` -> `403`/`CONNECT` denied (`TCP_DENIED`); `api.github.com`, `api.anthropic.com`, `api.openai.com`, `raw.githubusercontent.com` work |
| no direct route | `curl -m 5 --noproxy '*' https://api.anthropic.com` | times out (no route, and the SG has no 443 rule to the internet) |
| non-443 / plain HTTP | `curl -x $HTTPS_PROXY http://example.com`, `CONNECT example.com:22` | denied |
| AWS endpoints | `aws sts get-caller-identity`, `aws ec2 describe-instances`, `aws s3 ls s3://<artifacts>/artifacts/opt1/a1/` | work (STS/EC2 via the proxy, S3 via the gateway endpoint; if they hang, `NO_PROXY` in `/etc/ratchet/runner.env` is wrong for that service) |
| SSM | `cloud.runCommand(runner.id, ...)` | works (SSM agent uses the interface endpoints, not the proxy) |
| agent CLIs | start `claude -p` and `codex exec` with the runner env | both honor `HTTPS_PROXY` (the runner env sets `NODE_USE_ENV_PROXY=1` for Node's fetch; if a CLI still ignores the proxy, calls time out and it needs its own proxy flag) |
| package installs | `pnpm install` in the runner | works against `registry.npmjs.org` (a default); any other registry fails unless it is in `egress_proxy_extra_domains`; decide per project, do not allow `*` |
| failure mode | stop the proxy instance | runners lose model/GitHub access, SSM and S3 keep working; `tofu apply` replaces it |

## 10. DNS Firewall and the `bench-inputs/` lifecycle

`enable_dns_firewall` (default `true`) puts a Route 53 Resolver DNS Firewall rule group on the sandbox VPC. The allow-list
is built in `infra/aws/dns_firewall.tf`: the regional `ssm`, `ssmmessages`, `ec2messages`, `logs` and S3 names (the first four are
derived from the same list as the interface endpoints), the VPC's internal and reverse names, the agent domains
(`egress_proxy_domains` + `egress_proxy_extra_domains`, converted from squid syntax) and `dns_firewall_extra_domains`; a second rule
blocks everything else with NXDOMAIN. It fails closed (`firewall_fail_open = DISABLED`).

**Why one VPC and one list.** A rule group associates with a VPC, not a subnet, so it cannot be "bench only". Moving the bench to
its own VPC would mean a second set of the four interface endpoints (about $29/month each copy), a second S3 gateway endpoint and
new security-group and boundary references for every path agents use to reach SSM and S3. The shared-VPC consequence is accepted:
agent runners resolve only the allow-list too, with or without the proxy. That is a domain allow-list below squid, but it
changes the default behaviour of an unproxied runner from "HTTPS to anywhere": `registry.npmjs.org` is allowed by default (agents
and the deps snapshot install from it); other registries go in `dns_firewall_extra_domains` (`*.pythonhosted.org`, `pypi.org`, ...),
and the runner AMI must not
resolve other hosts at boot. If that is wrong for an environment, set `enable_dns_firewall=false` and accept DNS tunnelling
(see `bench.md`).

| Check | Command | Expected |
|---|---|---|
| plan | `tofu plan` | adds `aws_route53_resolver_firewall_rule_group.sandbox`, two `..._domain_list`, two `..._rule`, `..._rule_group_association.sandbox`, `..._firewall_config.sandbox`; changes the artifacts lifecycle configuration |
| association | `aws route53resolver list-firewall-rule-group-associations --vpc-id <vpc>` | one association, `Status: COMPLETE`, priority 101, `MutationProtection: DISABLED` |
| rules | `aws route53resolver list-firewall-rules --firewall-rule-group-id <id>` | priority 100 `ALLOW` (allowed list), priority 200 `BLOCK` with `BlockResponse: NXDOMAIN` (`*`) |
| allow-list | `aws route53resolver list-firewall-domains --firewall-domain-list-id <allowed>` | the regional service names, `*.s3.<region>.amazonaws.com`, `*.ec2.internal`, `*.<region>.compute.internal`, `*.<b>.<a>.in-addr.arpa`, `api.anthropic.com`, `github.com` + `*.github.com`, ...; never `*` or a bare suffix |
| bench blocked | on a bench instance: `getent hosts example.com`; `dig +short x.attacker.example`; `dig +short TXT a.b.example.org` | no answer / NXDOMAIN, as root and as `ratchet-bench` / `ratchet-holdout` |
| bench works | `getent hosts ssm.<region>.amazonaws.com`; `aws s3 cp s3://<artifacts>/bench-inputs/... .`; one SSM `RunCommand`; a full bench job | resolves (private endpoint addresses for `ssm`/`logs`); everything succeeds. If SSM registration breaks after enabling it, the SSM agent needs another name: read the blocked names (enable Resolver query logging on the VPC for a test run), add them to `dns_firewall_extra_domains`, and add the name to `dns_bench_domains` in `dns_firewall.tf` |
| agent runner | on a runner: `getent hosts api.anthropic.com`, `api.github.com`, `ec2.<region>.amazonaws.com`, `sts.<region>.amazonaws.com`; then `getent hosts registry.npmjs.org` and `pypi.org` | all but `pypi.org` resolve (npm is a default; other registries need `dns_firewall_extra_domains`); `claude -p` / `codex exec` and `git clone` over HTTPS still work |
| with proxy | `-var enable_egress_proxy=true` | the proxy instance still boots (`dnf install squid` needs `cdn.amazonlinux.com` and `*.s3.dualstack.<region>.amazonaws.com`, both added automatically) and `curl -x $HTTPS_PROXY https://api.anthropic.com` works |
| fail closed | `aws route53resolver get-firewall-config --resource-id <vpc>` | `FirewallFailOpen: DISABLED` |
| toggle | `tofu apply -var enable_dns_firewall=false` in a scratch sandbox | the association and rules are destroyed (no mutation-protection error), `getent hosts example.com` resolves again |
| cost | Cost Explorer after a day | about $0.60/month for the association plus $0.60 per million queries |

**The deps job and the allow-list.** `purpose: 'deps'` instances live in the agent subnet. `registry.npmjs.org` is in the default
`egress_proxy_domains` (squid and the DNS Firewall allow-list); another registry goes in `egress_proxy_extra_domains` with the proxy, or
`dns_firewall_extra_domains` without it. The DNS Firewall is per VPC, so an allowed name also *resolves* from a bench instance; the bench
still cannot *reach* it (no route out of the bench subnet; its security group and route table are untouched, and `tofu test` asserts
it). Verify from a deps instance that `registry.npmjs.org` resolves and the fetch works and that `example.com` does not; from a bench
instance, that `curl -m 5 https://registry.npmjs.org/` fails.

**`bench-inputs/` lifecycle.** The artifacts bucket has two rules: `expire-artifacts` (whole bucket, `artifacts_retention_days`, default 30)
and `expire-bench-inputs` (prefix `bench-inputs/`, `bench_inputs_retention_days`, default 7, plus noncurrent versions and a
1-day multipart abort). The bucket is not versioned today; the noncurrent rule is there so enabling versioning later cannot
make superseded staged code immortal.

| Check | Command | Expected |
|---|---|---|
| rules | `aws s3api get-bucket-lifecycle-configuration --bucket <artifacts>` | `expire-bench-inputs` `Enabled`, `Filter.Prefix: bench-inputs/`, `Expiration.Days: 7`, `NoncurrentVersionExpiration.NoncurrentDays: 7`; `expire-artifacts` unchanged |
| staged inputs | after a bench job: `aws s3api list-objects-v2 --bucket <artifacts> --prefix bench-inputs/` | the two tarballs (`code.tgz`, `bench.tgz`) of the job |
| expiry | the next day: `aws s3api head-object --bucket <artifacts> --key bench-inputs/<opt>/<job>/code.tgz` shows `Expiration: expiry-date=..., rule-id="expire-bench-inputs"` | the expiry date is 7 days (rounded to the next midnight UTC) after the upload; after that the object is gone |
| not too short | set `bench_inputs_retention_days=1` in a scratch sandbox and run a job that takes 3 hours | the job still finishes (S3 expires at midnight UTC following the age, so the minimum effective life is 24 h; the instance lifetime cap is `max_instance_lifetime_hours`, default 2) |
| other prefixes | `head-object` on an `artifacts/<opt>/<agent>/...` object | still the 30-day rule only |

## 11. Input-validation drill (tag/prefix injection)

The unit tests prove ids with `*`, `?`, `/`, `..` are rejected before any API call at every
`AwsCloudCompute` entry point, `TofuSandboxProvider`/`MockSandboxProvider` (sandbox id), and
`RemoteBenchRunner` (optimization id in the `bench-inputs/` prefix). For real, check that nothing else in the
app builds those names from user input: grep for `ratchet/${`, `artifacts/${` and `/ratchet/` in
`packages/*/src` and confirm each id passes `assertValidId` first. And on the account: create an agent
role by hand with `ratchet:agent` = `*` and confirm the boundary's `ratchet:agent` match does not widen
(the principal tag is substituted literally in `Resource`, but `StringLike`/ARN patterns do glob, which is
why ids must be validated).

## Mock/real parity checks

`MockCloudCompute`, `MockSandboxProvider` and `StaticPriceBook` are used by simulation mode. After
steps 2-3, run the same scenario against both and diff the observable results: identity
refs shape (`arn:aws:iam::<acct>:role/ratchet/agent/ratchet-agent-<opt>-<agent>`), tag-derived
roles from `listTagged`, idempotent create/terminate/delete, revoke of a missing role throwing
(`NoSuchEntity`), and `launchRunner` with a deleted identity failing with
`Invalid IAM Instance Profile`. Known intentional differences: the mock's `runCommand` executes
an injected local handler, and its instances boot instantly unless `bootDelayMs` is set.
