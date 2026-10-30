# Sandbox contract

Every cloud module under `infra/<cloud>/` (v0: `infra/aws`) implements this
contract. The app (`SandboxProvider` in `packages/core`) depends only on the
inputs and outputs below, never on provider-specific resources.

A *sandbox* is a long-lived, dedicated, per-user environment (one cloud
account/project) in which agents run and in which a trusted bench runner
re-measures their candidates. Everything agent-scoped (per-agent identities,
runner machines, secrets) is created at runtime by the app, *inside* the
guard rails this module provisions.

## Inputs

| Input | Type | Meaning |
|---|---|---|
| `sandbox_id` | string | Stable id of the sandbox. Lowercase letters, digits, `-`; 2-32 chars. Prefixes every named resource so several sandboxes can coexist in one account. |
| `region` | string | Cloud region everything is created in. Agents cannot create resources elsewhere. |
| `allowed_instance_types` | list(string) | Machine types an agent may launch for experiments. Anything else is denied. Non-empty. |
| `max_instances_per_agent` | number | Maximum concurrently running experiment machines per agent (default 4). **Advisory to the cloud IAM layer** - enforced by the app reaper (see below). Echoed in the `limits` output. |
| `max_instance_lifetime_hours` | number | Maximum lifetime of any agent-launched machine (default 2). **Enforced by the app reaper**, not by IAM. Echoed in the `limits` output. |
| `tags` | map(string) | Extra tags applied to every resource created by the module (e.g. owner, cost centre). The reserved `ratchet:*` keys are set by the module/app and cannot be overridden. |

Cloud-specific *optional* inputs (e.g. AWS `vpc_cidr`, `enable_egress_proxy`) are allowed
as long as they have defaults and the app can run without them.

## Outputs

| Output | Meaning |
|---|---|
| `artifacts_bucket` | Name of the object store agents read/write. One prefix per agent: `artifacts/<opt>/<agent>/`. Holds event streams and build outputs. |
| `sealed_bucket` | Name of the object store holding hidden traces. Readable **only** by the confirm identity, and only under its holdout prefix. |
| `agent_boundary_ref` | Reference to the permission cap that every per-agent identity must be created under (AWS: permission boundary policy ARN). |
| `eval_identity_ref` | The identity trusted bench machines run as in the `baseline` and `bench` phases (AWS: object with `role_arn`, `role_name`, `instance_profile_arn`, `instance_profile_name`). **No access to the sealed store.** |
| `confirm_identity_ref` | The identity a bench machine runs as in the `confirm` phase only; the sole identity that can read `sealed_bucket` (same shape as `eval_identity_ref`). |
| `deps_identity_ref` | The identity of the dependency-snapshot and container-image builder (`purpose=deps`; same shape as `eval_identity_ref`). It runs in the agent subnet (the only place with egress), never executes candidate code, may read only the staged base tree (`bench-inputs/*/deps-base.tgz`) and `bench-images/`, and write only `bench-deps/` and `bench-images/` in the artifacts store, and has **no access to the sealed store** (explicitly denied). Optional for sandboxes created before dependency snapshots: without it the app cannot build snapshots and fails closed. |
| `network_ref` | Where machines are launched (AWS: object with `vpc_id`, `subnet_id` (agent runners), `bench_subnet_id` (bench machines; no route to the internet), `security_group_ids.agent_runner`, `security_group_ids.bench`, `egress_proxy_url` (null unless the optional allow-list proxy is enabled)). |
| `agent_role_path` | The path/name-space under which per-agent identities MUST be created (AWS: `/ratchet/agent/`). The sealed-store deny matches on it. |
| `limits` | Echo of `max_instances_per_agent` and `max_instance_lifetime_hours` so the app reads limits from the sandbox, not from its own config. |

## Identity model

Per-agent identities are created by the app at runtime, not by this module.
To be governed by the sandbox they MUST:

1. live under `agent_role_path`,
2. have `agent_boundary_ref` attached as their permission cap,
3. carry the identity attributes `ratchet:opt=<optimization id>` and
   `ratchet:agent=<agent id>` (AWS: IAM role tags, surfaced as principal
   tags). Ids are restricted to `[a-z0-9-]` by the app, because they are
   substituted into resource names and policy patterns.

Everything launched by an agent must carry `ratchet:opt` and `ratchet:agent`
equal to the agent's own attributes.

## Invariants every cloud module must satisfy

An implementation that cannot satisfy one of these is not a valid sandbox.

1. **Sealed isolation.** No agent identity can read, list, write, or delete
   anything in `sealed_bucket`, whatever its identity-level permissions say.
   Enforced twice: an explicit deny on the store *and* an explicit deny in the
   agent boundary. The same explicit deny covers the `eval_identity_ref`
   identity: only `confirm_identity_ref` can read the store, and only the
   holdout prefix.
2. **Eval isolation.** The eval and confirm identities are outside
   `agent_role_path`; agents cannot assume them, pass them to a machine, or
   alter them.
3. **Per-agent storage.** An agent can touch only
   `artifacts/<its opt>/<its agent>/` in `artifacts_bucket` - not other agents,
   not other optimizations, not the bucket root listing.
4. **Per-agent secrets.** An agent can read only secrets under
   `/ratchet/<its opt>/<its agent>/`.
5. **Tagged compute.** An agent can launch machines only if they are tagged
   with its own `ratchet:opt` / `ratchet:agent`, only of an allow-listed type,
   only inside the sandbox network, and only with hardened metadata access
   (AWS: IMDSv2 required). It can start/stop/terminate only machines carrying
   its own tags. Launched machines get no identity of their own (no privilege
   escalation through instance roles). They cannot launch into the bench or
   proxy subnets nor with the bench, endpoint or proxy security groups.
6. **No control-plane access.** Agents have no access to identity management,
   organisation, billing/cost, or the ability to assume other roles. The
   boundary cannot be detached or edited by the agent.
7. **Fail closed.** An identity lacking the `ratchet:opt` / `ratchet:agent`
   attributes is denied everything.
8. **Network.** Agent runner machines have no inbound access and egress only to
   HTTPS and name resolution (with the optional proxy: only to the proxy, the
   private service endpoints and the object store, and the allow-list is by
   domain). Bench machines accept no inbound traffic from agent runners and
   have **no route to the internet**: they reach only each other, the object
   store (through an endpoint whose policy admits just the two sandbox
   buckets) and the machine-management/log endpoints. Candidate code is
   delivered by pre-staging in the artifacts store, never fetched from a git
   host. The module owns the network (a dedicated VPC); the account's default
   VPC is never used.
9. **No public data.** Both stores block public access, encrypt at rest, and
   accept only encrypted transport.
10. **Reversible.** Destroying the module removes everything it created, with
    no manual cleanup beyond runtime-created resources (below).
11. **No secrets in state.** The module never receives or stores model keys or
    tokens.
12. **Offline bench, online dependency fetch.** Bench machines never get a
    route out, so package installs cannot download anything. Dependencies reach
    the bench as a *snapshot* (a package store or cache) that a separate
    `purpose=deps` machine in the agent subnet builds from the trusted base
    ref's lockfile and stores under `bench-deps/` in the artifacts store. That
    machine runs the package manager's fetch step as an unprivileged,
    metadata-blocked user (no lifecycle scripts, no candidate code) under an
    identity that can write only `bench-deps/` and `bench-images/`; bench
    identities can read snapshots and images but never write them. A candidate
    whose lockfile differs from the base ref is blocked, not retried with
    network access. The same holds for the repo's container image
    (`environment.image`): it is built once from the base ref on the egress
    machine, saved under `bench-images/` (content-addressed archive, manifest
    last) and loaded offline on the bench; a candidate that changes its
    Dockerfile / devcontainer or a file it copies is blocked ("environment
    changes are not allowed in v0"). A custom module (`infra:`) must satisfy
    every output of this contract; the app checks them after apply
    (`checkSandboxContract`).
13. **Least identity per bench phase.** The `baseline`/`bench` phases run under
    an identity with no access to the sealed store; the `confirm` phase runs
    under a separate identity that can read the holdout prefix. The app picks
    the identity from the job's phase and fails closed when the confirm
    identity is not configured. Because candidate code runs on the same
    machine, the bench image must additionally run candidate code as an
    unprivileged OS user that cannot reach instance metadata
    (`docs/real-env/bench.md`).

## Limits IAM cannot express (app-enforced)

| Limit | Why not in IAM | Enforcement |
|---|---|---|
| `max_instances_per_agent` | IAM has no counting condition for `RunInstances`. | App reaper polls tagged instances per agent, terminates excess and trips the kill switch on repeat offenders. |
| `max_instance_lifetime_hours` | IAM has no time-to-live condition. | App reaper terminates tagged instances older than the limit; runners also enforce locally. |
| Terminate-on-shutdown (AWS `InstanceInitiatedShutdownBehavior=terminate`) | No IAM condition key exists for this `RunInstances` parameter. | Runner image/CLI wrapper sets it; reaper terminates `stopped` agent instances. |
| Spend budget | IAM is not cost aware. | App cost meter + kill switch (deny-all on the agent identity). |
| Outbound destination allow-list (by domain) for agent runners | Security groups filter by IP/port only. | Optional (`enable_egress_proxy`, default off): a forward proxy allow-lists domains and the runners' route to the internet is removed, so the allow-list is enforced by routing, not only by a security group. Without it, runners have HTTPS egress to anywhere. |
| DNS exfiltration from bench machines | Security groups do not filter traffic to the VPC resolver. | Residual risk; a DNS firewall on the VPC is the follow-up (`docs/real-env/bench.md`). |
| Which OS user can reach instance metadata | IAM/network groups see the instance, not a Unix user. | The bench image blocks 169.254.169.254 for the unprivileged bench user (owner-match firewall rule) and the bench runner refuses to run otherwise. |

## Runtime-created resources (not in the module)

Per-agent identity under `agent_role_path` + attached boundary + instance
profile; agent runner machines; experiment machines launched by agents; secret
parameters under `/ratchet/<opt>/<agent>/`; the kill-switch deny-all policy
attachment; bench machines (one fresh machine per job) and the staged candidate
archives under `bench-inputs/`. See `infra/README.md`.
