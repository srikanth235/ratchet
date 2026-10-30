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

Cloud-specific *optional* inputs (e.g. AWS `vpc_id`, `subnet_id`) are allowed
as long as they have defaults and the app can run without them.

## Outputs

| Output | Meaning |
|---|---|
| `artifacts_bucket` | Name of the object store agents read/write. One prefix per agent: `artifacts/<opt>/<agent>/`. Holds event streams and build outputs. |
| `sealed_bucket` | Name of the object store holding hidden traces. Readable **only** by the eval identity. |
| `agent_boundary_ref` | Reference to the permission cap that every per-agent identity must be created under (AWS: permission boundary policy ARN). |
| `eval_identity_ref` | The identity trusted bench machines run as (AWS: object with `role_arn`, `role_name`, `instance_profile_arn`, `instance_profile_name`). |
| `network_ref` | Where machines are launched (AWS: object with `vpc_id`, `subnet_id`, `security_group_ids.agent_runner`, `security_group_ids.bench`). |
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
   agent boundary.
2. **Eval isolation.** The eval identity is outside `agent_role_path`; agents
   cannot assume it, pass it to a machine, or alter it.
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
   escalation through instance roles).
6. **No control-plane access.** Agents have no access to identity management,
   organisation, billing/cost, or the ability to assume other roles. The
   boundary cannot be detached or edited by the agent.
7. **Fail closed.** An identity lacking the `ratchet:opt` / `ratchet:agent`
   attributes is denied everything.
8. **Network.** Agent runner machines have no inbound access and egress only to
   HTTPS and name resolution. Bench machines accept no inbound traffic from
   agent runners.
9. **No public data.** Both stores block public access, encrypt at rest, and
   accept only encrypted transport.
10. **Reversible.** Destroying the module removes everything it created, with
    no manual cleanup beyond runtime-created resources (below).
11. **No secrets in state.** The module never receives or stores model keys or
    tokens.

## Limits IAM cannot express (app-enforced)

| Limit | Why not in IAM | Enforcement |
|---|---|---|
| `max_instances_per_agent` | IAM has no counting condition for `RunInstances`. | App reaper polls tagged instances per agent, terminates excess and trips the kill switch on repeat offenders. |
| `max_instance_lifetime_hours` | IAM has no time-to-live condition. | App reaper terminates tagged instances older than the limit; runners also enforce locally. |
| Terminate-on-shutdown (AWS `InstanceInitiatedShutdownBehavior=terminate`) | No IAM condition key exists for this `RunInstances` parameter. | Runner image/CLI wrapper sets it; reaper terminates `stopped` agent instances. |
| Spend budget | IAM is not cost aware. | App cost meter + kill switch (deny-all on the agent identity). |
| Outbound destination allow-list (by domain) | Security groups filter by IP/port only. | Egress is limited to 443/DNS; a domain-filtering proxy/firewall is a later hardening. |

## Runtime-created resources (not in the module)

Per-agent identity under `agent_role_path` + attached boundary + instance
profile; agent runner machines; experiment machines launched by agents; secret
parameters under `/ratchet/<opt>/<agent>/`; the kill-switch deny-all policy
attachment. See `infra/README.md`.
