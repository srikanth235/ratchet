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
| `AwsCloudCompute` | Exact request params: role path, boundary, tags, IMDSv2, terminate-on-shutdown, profile, deletion order, SSM paths | IAM eventual-consistency retry is enough, the boundary really permits/denies as intended, SSM agent reachability |
| `AwsPriceBook` | Filters sent, price parsing, cache | Filters match real Pricing API attribute values |
| `S3ArtifactStore` | Request params, paging, NoSuchKey | Bucket policy / SSE / the agent prefix restriction |
| `CostMeter`, `BudgetGuard`, `Reaper` | Math, rules, idempotency (against `MockCloudCompute`) | Tag filters return what we expect; `StateTransitionReason` parsing; real instance lifetimes |

## 0. Prerequisites

- A dedicated AWS account with an SSO profile; `export AWS_PROFILE=ratchet-sandbox AWS_REGION=us-east-1`.
- A pinned `tofu` binary (`tofu version`) and this repo built: `pnpm install && pnpm build`.
- A runner AMI and a bench AMI (Node, git, agent CLIs, AWS CLI, SSM agent, a `ratchet` binary
  on PATH providing `ratchet runner` / `ratchet bench-agent`). Until those exist, use any
  SSM-enabled Amazon Linux AMI: instances will launch, tag, accrue cost and be SSM-reachable,
  which is all steps 2-6 need.
- Billing alarms on the account (the kill switch is a backstop, not a substitute).

Run the snippets below from a scratch script (`node --input-type=module -e '...'` or a
`.mjs` file) importing from `packages/aws/dist/index.js`.

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
| plan | `sandbox.plan(input)` | 8 resources + `aws_iam_role_policy.bench_runner` with action `create`; summary `Plan: 9 to add, 0 to change, 0 to destroy.` (count may differ by the extra policy/public-access-block resources; every `create`) |
| state location | `ls /tmp/ratchet-sandboxes/rt-test` | `vars.tfvars.json`, `plan.tfplan`, `.terraform-data/`; and **no** `terraform.tfstate` or `.terraform*` in `infra/aws/` (`git status infra/` clean) |
| apply | `sandbox.apply(input)` | resolves with `artifactsBucket`, `sealedBucket`, `agentBoundaryRef` (ARN), `agentRolePath: '/ratchet/agent/'`, network ids, `limits` `{4, 2}`; `terraform.tfstate` now present under the sandbox dir; `plan.tfplan` removed |
| idempotent | `sandbox.plan(input)` again | every change `no-op` |
| outputs | `sandbox.outputs('rt-test')` (new process) | same values; `region` restored from `vars.tfvars.json`; unknown id -> `null` |
| console check | `aws s3api get-public-access-block --bucket <sealed>` | all four true |

**Unverified assumption to check first:** we pass `-state=<path>` to both `plan` and
`apply <planfile>`. If `tofu apply` rejects `-state` with a saved plan, drop it from
`apply` in `TofuSandboxProvider.apply` (the plan file records the state path). Also
confirm `init -backend=false` with `TF_DATA_DIR` re-initialises cleanly on each call.

**Destroy guard** (do this before step 7's teardown): with a runtime agent role present
(step 2), `sandbox.destroy(input)` must throw `TeardownBlockedError` listing the role and
must not run `tofu destroy` (`aws s3 ls` still shows the buckets).

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
| launch | `aws ec2 describe-instances --instance-ids <id>` | `MetadataOptions.HttpTokens=required`, `InstanceInitiatedShutdownBehavior` = `terminate` (`aws ec2 describe-instance-attribute --attribute instanceInitiatedShutdownBehavior`), subnet = `network_ref.subnet_id`, SG = agent_runner SG, profile = agent profile, tags `ratchet:opt/agent/role=agent_runner/sandbox`, volume tagged too |
| IAM propagation | launch immediately after `createAgentIdentity` | succeeds, possibly after retries (log the number; if >6 retries are needed, raise `iamPropagationRetries`) |
| user-data | `aws ec2 describe-instance-attribute --attribute userData` (base64 decode) | contains refs (`RATCHET_SECRET_REFS`), **no secret values**, `shutdown -h +120` |
| list | `cloud.listTagged('opt1')` | the runner with `role: 'agent_runner'`, `state` pending->running |
| SSM | `cloud.runCommand(runner.id, { script: 'echo $X; id -un', env: { X: "a'b" } })` | `{exitCode:0, stdout:"a'b\n..."}`; `exit 7` -> `exitCode 7`; a `sleep` beyond `timeoutSeconds` -> exit 124. Note: SSM truncates stdout at 24000 chars |
| bench | `cloud.launchBenchInstance({ optimizationId:'opt1', instanceType:'c7i.2xlarge', imageRef:'ami-...', purpose:'baseline' })` | eval instance profile, bench SG, no `ratchet:agent` tag, role=`bench` |
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

## Mock/real parity checks

`MockCloudCompute`, `MockSandboxProvider` and `StaticPriceBook` are used by simulation mode. After
steps 2-3, run the same scenario against both and diff the observable results: identity
refs shape (`arn:aws:iam::<acct>:role/ratchet/agent/ratchet-agent-<opt>-<agent>`), tag-derived
roles from `listTagged`, idempotent create/terminate/delete, revoke of a missing role throwing
(`NoSuchEntity`), and `launchRunner` with a deleted identity failing with
`Invalid IAM Instance Profile`. Known intentional differences: the mock's `runCommand` executes
an injected local handler, and its instances boot instantly unless `bootDelayMs` is set.
