# infra

Cloud-neutral sandbox contract and the AWS implementation.

```
contract.md   what every sandbox module must accept, produce and guarantee
aws/          OpenTofu module (v0)
  policies/   IAM/bucket policy templates (*.json.tftpl), the single source of truth
  tests/      `tofu test` files (mocked provider)
images/       Packer definitions of the runner and bench machine images (AMIs) live mode launches; see images/README.md
tests/        vitest suite: renders the policies, simulates IAM decisions, checks module wiring and the image scripts
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
  with `-var`/`-var-file`. Optional AWS inputs: `vpc_cidr` (default
  `10.42.0.0/16`; the module always creates its own VPC and never touches the
  default one), `availability_zone`, `artifacts_retention_days`,
  `force_destroy`, `sealed_confirm_prefix` (default `holdout/`: the only part of
  the sealed bucket the confirm runner can read), and the egress-proxy switches
  `enable_egress_proxy` (default `false`), `egress_proxy_instance_type`,
  `egress_proxy_domains`, `egress_proxy_extra_domains`, the DNS firewall
  switches `enable_dns_firewall` (default `true`) and `dns_firewall_extra_domains`,
  and `bench_inputs_retention_days` (default 7: expiry of the staged `bench-inputs/`). Removed:
  `vpc_id`, `subnet_id`, `bench_egress_cidrs`.
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
| `aws_s3_bucket.artifacts` (+ public-access block, ownership, SSE, lifecycle: 30 days, `bench-inputs/` after `bench_inputs_retention_days`) | per-agent prefixes; `bench-inputs/` (staged candidates) and `bench-results/` |
| `aws_s3_bucket.sealed` (+ public-access block, ownership, SSE, versioning) | hidden traces |
| `aws_s3_bucket_policy.sealed_deny_agents` | deny agent roles (path + tag), deny the bench runner, TLS only |
| `aws_iam_policy.agent_boundary` | cap for every agent role |
| `aws_iam_role.bench_runner`, `aws_iam_instance_profile.bench_runner` (+ `aws_iam_role_policy.bench_runner`) | bench identity for `baseline`/`bench` phases: reads `bench-inputs/`, writes `bench-results/`, **no sealed access** |
| `aws_iam_role.confirm_runner`, `aws_iam_instance_profile.confirm_runner` (+ `aws_iam_role_policy.confirm_runner`) | the same plus read of the holdout prefix of the sealed bucket; `purpose: 'confirm'` only |
| `aws_vpc.sandbox`, `aws_internet_gateway.sandbox`, `aws_default_security_group.sandbox`, route tables/routes/associations | dedicated VPC; the private route table has no default route |
| `aws_subnet.bench` (private), `aws_subnet.agent` (public, or private with the proxy) | where bench machines / agent runners launch |
| `aws_vpc_endpoint.s3` (gateway, policy: the two sandbox buckets only), `aws_vpc_endpoint.interface["ssm" \| "ssmmessages" \| "ec2messages" \| "logs"]`, `aws_security_group.endpoints` | the only ways out of the bench subnet |
| `aws_security_group.agent_runner`, `aws_security_group.bench` | network |
| `aws_route53_resolver_firewall_*` (rule group, 2 domain lists, 2 rules, association, config; **on unless `enable_dns_firewall = false`**) | DNS Firewall on the VPC: allow the S3/SSM/ssmmessages/ec2messages/logs names of the region, internal names and the agent domains, NXDOMAIN for the rest (stops DNS tunnelling) |
| `aws_subnet.proxy`, `aws_security_group.egress_proxy`, `aws_instance.egress_proxy` (**only with `enable_egress_proxy`**) | domain allow-list proxy for agent runners |

### Costs of the network pieces (us-east-1 list prices; check current pricing)

| Piece | Always on | Approx. monthly |
|---|---|---|
| 4 interface endpoints (ssm, ssmmessages, ec2messages, logs), one AZ | yes | 4 x $0.01/h x 730 h = **~$29** + $0.01/GB |
| S3 gateway endpoint, internet gateway, VPC, subnets, routes | yes | $0 |
| Egress proxy: t4g.nano + public IPv4 + 8 GB gp3 | only with `enable_egress_proxy` | ~$3.1 + ~$3.65 + ~$0.65 = **~$7.5** + data out |
| DNS Firewall: 1 rule group association | yes (unless disabled) | $0.60 + $0.60 per million queries |
| (not used) AWS Network Firewall | no | ~$0.395/h per endpoint = ~$290 + $0.065/GB |

The proxy is a single squid instance because it is ~40x cheaper and trivially
inspectable; the price is no failover (if it dies, runners lose the model APIs
until it is replaced: `tofu apply` recreates it) and no TLS inspection (the
allow-list matches the CONNECT hostname). Destroy the sandbox when idle to stop
the endpoint charges.

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
   - agents can launch experiment instances only into the agent subnet
     (`network_ref.subnet_id`), never into the bench or proxy subnet nor with the
     bench / endpoint / proxy security groups (explicit deny in the boundary).
   Without the tags the boundary denies everything; without the boundary the
   sealed bucket still denies the role by path.
2. **Agent runner EC2 instance** using `network_ref.subnet_id` and the
   `agent_runner` security group, with that instance profile, IMDSv2 required.
   When `network_ref.egress_proxy_url` is set the launcher exports it as
   `HTTPS_PROXY` (with `NO_PROXY` for IMDS and the private service endpoints).
3. **Bench instances**, one fresh machine per job in `network_ref.bench_subnet_id` with the
   `bench` security group. `purpose: baseline|bench` -> `eval_identity_ref` profile (no sealed
   access); `purpose: confirm` -> `confirm_identity_ref` profile. The bench image's user-data
   creates the unprivileged `ratchet-bench` user and blocks its access to 169.254.169.254.
   The candidate and `bench.ref` are uploaded to `bench-inputs/<opt>/<job>/` first (the
   machine has no internet and no git credentials).
4. **Experiment instances** launched by agents (via the AWS CLI on the runner)
   inside the boundary, tagged `ratchet:opt` / `ratchet:agent`.
5. **SSM SecureString parameters** at `/ratchet/<opt>/<agent>/...` (model API
   key, repo-scoped GitHub token). The bench identities have no secrets access at all: the candidate is
   pre-staged, so there is no repo token on a bench machine.
6. **Kill switch**: attach a deny-all inline policy to the agent role.
7. **Reaper**: enforces `max_instances_per_agent`, `max_instance_lifetime_hours`
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
