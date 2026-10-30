# infra

Cloud-neutral sandbox contract and the AWS implementation.

```
contract.md   what every sandbox module must accept, produce and guarantee
aws/          OpenTofu module (v0)
  policies/   IAM/bucket policy templates (*.json.tftpl), the single source of truth
  tests/      `tofu test` files (mocked provider)
tests/        vitest suite: renders the policies, simulates IAM decisions, checks module wiring
```

## How the app drives it

The app (`TofuRunner`) bundles a pinned `tofu` binary and uses the user's AWS
profile / SSO login. The sandbox must be a **dedicated account**.

```sh
cd infra/aws
tofu init -backend=false          # first time; local state, see below
tofu plan  -out=plan.tfplan -var sandbox_id=demo -var region=us-east-1 \
           -var 'allowed_instance_types=["t3.medium","c7i.large"]'
tofu show  -json plan.tfplan      # the app renders this diff before asking to apply
tofu apply plan.tfplan
tofu output -json                 # contract outputs, parsed by AWSSandboxProvider
tofu destroy -var sandbox_id=demo -var region=us-east-1   # teardown
```

- **Variables** are the contract inputs (`contract.md`); the app passes them
  with `-var`/`-var-file`. Optional AWS inputs: `vpc_id`, `subnet_id` (default
  VPC and its first subnet otherwise), `artifacts_retention_days`,
  `force_destroy`, `bench_egress_cidrs`.
- **State** is local, one directory per sandbox:
  `~/Library/Application Support/Ratchet/sandboxes/<id>/terraform.tfstate`
  (`-state=<path>`, and run with `TF_DATA_DIR` pointing at the same directory).
  A remote S3 backend can follow later. State contains no secrets: the module
  never sees model keys or tokens.
- **Before `destroy`** the app must first delete runtime-created resources
  (below), otherwise the boundary policy cannot be deleted while roles still
  have it attached. `force_destroy = true` (default) lets destroy remove
  non-empty buckets, including versions.

## Created by OpenTofu

| Resource | |
|---|---|
| `aws_s3_bucket.artifacts` (+ public-access block, ownership, SSE, lifecycle) | per-agent prefixes |
| `aws_s3_bucket.sealed` (+ public-access block, ownership, SSE, versioning) | hidden traces |
| `aws_s3_bucket_policy.sealed_deny_agents` | deny agent roles (path + tag), TLS only |
| `aws_iam_policy.agent_boundary` | cap for every agent role |
| `aws_iam_role.bench_runner`, `aws_iam_instance_profile.bench_runner` (+ `aws_iam_role_policy.bench_runner`) | trusted bench identity |
| `aws_security_group.agent_runner`, `aws_security_group.bench` | network |

## Created at runtime by the app (NOT in OpenTofu)

These are short-lived and per optimization, so they use the AWS SDK
(`CredentialMinter`, `RunnerLauncher`) and are tracked and reaped by the app:

1. **Per-agent IAM role** under path `/ratchet/agent/` (`agent_role_path`
   output), with:
   - `PermissionsBoundary = agent_boundary_ref`
   - tags `ratchet:opt=<opt id>` and `ratchet:agent=<agent id>` (ids restricted
     to `[a-z0-9-]`; the policies substitute them into resource patterns)
   - trust policy for `ec2.amazonaws.com`, plus an identity policy that grants
     what the agent needs (the boundary only caps it), and an instance profile.
   Without the tags the boundary denies everything; without the boundary the
   sealed bucket still denies the role by path.
2. **Agent runner EC2 instance** using `network_ref.subnet_id` and the
   `agent_runner` security group, with that instance profile, IMDSv2 required.
3. **Experiment instances** launched by agents (via the AWS CLI on the runner)
   inside the boundary, tagged `ratchet:opt` / `ratchet:agent`.
4. **SSM SecureString parameters** at `/ratchet/<opt>/<agent>/...` (model API
   key, repo-scoped GitHub token). Bench secrets live at `/ratchet/bench/...`.
5. **Kill switch**: attach a deny-all inline policy to the agent role.
6. **Reaper**: enforces `max_instances_per_agent`, `max_instance_lifetime_hours`
   and terminate-on-shutdown (see `limits` output); none are expressible in IAM.

Teardown order: kill switch -> terminate tagged instances -> delete parameters
-> detach/delete agent roles and profiles -> `tofu destroy`.

## Verifying without AWS

```sh
pnpm --filter @ratchet/infra-tests test          # policy rendering + IAM simulator + wiring checks
cd infra/aws && tofu fmt -check && tofu init -backend=false && tofu validate && tofu test
```

The vitest suite renders the same `*.json.tftpl` files OpenTofu uses and
asserts the boundary's intent with a small IAM evaluator (`iam-sim.ts`). It
models only the condition operators we use and evaluates one policy at a time;
it is not a substitute for the IAM Policy Simulator in the nightly test
account (PLAN Phase 2 exit criteria).
