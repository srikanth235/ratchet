import { DEFAULT_ENVIRONMENT_INFRA_DIR, ENVIRONMENT_SCOPES, type EnvironmentScope } from '../config/schema.js';
import { INSTANCE_USD_PER_HOUR } from './cost.js';
import { ENVIRONMENT_OBJECT_PREFIX, MACHINE_KEYS, MAX_ENVIRONMENT_MACHINES, SANDBOX_BASE_MODULE_NAME, SANDBOX_BASE_MODULE_SOURCE } from './validate.js';
import { ENVIRONMENT_ROLES } from './types.js';

/**
 * The brief a setup agent (the user's Claude Code / Codex, run locally in a checkout of the repo) is given. The agent is asked
 * to size an environment to the task and write ONLY what is needed; Ratchet validates the result (`validateEnvironment`), so
 * the brief states the rules the validator enforces to save round trips, but the brief is not what makes the draft safe.
 */

/** The marker the agent ends its final message with, followed by one JSON object (`AgentAnswer`). */
export const ANSWER_MARKER = 'RATCHET_ENVIRONMENT';

export interface AgentAnswer {
  scope: EnvironmentScope;
  /** One or two plain sentences: why this much infrastructure. Shown to the user. */
  reason: string;
}

/** The text the agent must keep to when it describes its choice. Exposed so the mock and the tests produce the same shape. */
export function formatAgentAnswer(a: AgentAnswer): string {
  return `${ANSWER_MARKER} ${JSON.stringify(a)}`;
}

/** Reads the agent's final message. Tolerant: the marker may be anywhere and the JSON may be fenced; a bad answer is `null`. */
export function parseAgentAnswer(text: string): AgentAnswer | null {
  const at = text.lastIndexOf(ANSWER_MARKER);
  if (at < 0) return null;
  const rest = text.slice(at + ANSWER_MARKER.length);
  const start = rest.indexOf('{');
  if (start < 0) return null;
  let depth = 0;
  let inStr = false;
  for (let i = start; i < rest.length; i++) {
    const c = rest[i]!;
    if (inStr) {
      if (c === '\\') i++;
      else if (c === '"') inStr = false;
    } else if (c === '"') inStr = true;
    else if (c === '{') depth++;
    else if (c === '}' && --depth === 0) {
      try {
        const o = JSON.parse(rest.slice(start, i + 1)) as { scope?: unknown; reason?: unknown };
        if (typeof o.scope !== 'string' || !(ENVIRONMENT_SCOPES as readonly string[]).includes(o.scope)) return null;
        return { scope: o.scope as EnvironmentScope, reason: typeof o.reason === 'string' ? o.reason.trim() : '' };
      } catch {
        return null;
      }
    }
  }
  return null;
}

/** The canonical shape of `main.tf`, shown to the agent (and used by the mock drafter). */
export const MAIN_TF_SKELETON = `module "${SANDBOX_BASE_MODULE_NAME}" {
  source  = "${SANDBOX_BASE_MODULE_SOURCE}"
  ratchet = var.ratchet

  machines = {
    api = {
      role          = "app"
      label         = "API"
      detail        = "the service in the repo's container"
      instance_type = "c7i.2xlarge"
      user_data     = file("\${path.module}/scripts/api.sh")
    }
    load = {
      role          = "load"
      label         = "Load generator"
      detail        = "k6, on its own host"
      instance_type = "c7i.large"
      user_data     = file("\${path.module}/scripts/load.sh")
    }
  }
}
`;

export interface SetupBriefInput {
  /** What the user wants optimized, in their words. */
  goal: string;
  /** The bench machine the repo's ratchet.yaml / template pins, when known (default for `single`). */
  hardware?: string;
  /** The run's time limit in seconds, so the agent can weigh cost. */
  timeLimitSeconds?: number;
}

/** The first message to the setup agent. */
export function buildSetupBrief(input: SetupBriefInput): string {
  const types = Object.keys(INSTANCE_USD_PER_HOUR).join(', ');
  const hours = input.timeLimitSeconds ? `${(input.timeLimitSeconds / 3600).toFixed(1)} h` : '2 h';
  return `You are the setup agent for Ratchet, a tool that lets coding agents optimize a metric of this repository and keeps only changes a trusted evaluator confirms. Your job is to design the ENVIRONMENT the optimization is measured in, before any agent starts. You do not optimize anything and you do not change the code under test.

## The goal
${input.goal.trim()}

## Read first
Read the repository (README, build and test files, the service entry points, ratchet.yaml, RATCHET.md, the evaluator under bench/ or eval/) so you know what has to run for the goal's metric to be measured honestly.

## Decide the scope, as small as the task allows
- \`none\`: pure code or tests. Nothing needs to be deployed; the evaluator's container is enough. Write ONLY the \`environment.scope: none\` line.
- \`single\`: one machine is enough (a CLI, a build, one service that needs nothing else). Pin the hardware with \`environment.machine\`. You may add a Dockerfile under ${DEFAULT_ENVIRONMENT_INFRA_DIR}/ and point \`environment.image\` at it.
- \`full\`: the metric depends on other components (a database with seeded data, a cache, a separate load generator, other services). Write OpenTofu under ${DEFAULT_ENVIRONMENT_INFRA_DIR}/.
Choose \`full\` only when a single machine would not measure the metric; every machine costs money while it runs. The run is limited to ${hours}.

## What you may write (anything else is rejected)
1. \`ratchet.yaml\`: add or change ONLY the \`environment:\` block (\`scope\`, and when needed \`machine\`, \`image\`, \`region\`). Do not touch objective, evaluator, guardrails, protected, agent or any egress hosts.
2. Files under \`${DEFAULT_ENVIRONMENT_INFRA_DIR}/\`: \`main.tf\`, scripts, a Dockerfile, seed data.
Do not edit source code, tests, the evaluator or lockfiles.

## The base module (the only module you may call)
Ratchet ships \`${SANDBOX_BASE_MODULE_SOURCE}\`. It brings the network isolation, the evaluation-only identity, the sealed results and the budget tags; you do not create networks, security groups, IAM, buckets or public addresses, and you do not declare providers, backends or variables named \`ratchet\`. You describe machines as a LITERAL map; each entry may only have: ${MACHINE_KEYS.join(', ')}.
- \`role\`: ${ENVIRONMENT_ROLES.join(' | ')}
- \`instance_type\`: a literal. Fixed-performance types only for app, load and evaluator (no burstable t-types). Known types and prices: ${types}.
- \`user_data\`: a script, from a file in the draft: \`file("\${path.module}/scripts/x.sh")\`.
- At most ${MAX_ENVIRONMENT_MACHINES} machines. Allowed extra resource: \`aws_s3_object\` for seed data, \`bucket = module.sandbox.artifacts_bucket\`, key under \`${ENVIRONMENT_OBJECT_PREFIX}\`, \`tags = module.sandbox.tags\`.

\`main.tf\` looks like this:

\`\`\`hcl
${MAIN_TF_SKELETON}\`\`\`

## Hard rules
- The measuring machines have NO internet. Scripts must not curl, wget, pip/npm/apt install, docker pull or git clone: everything they need comes from the repo's container image or the seed data you ship.
- Hardware is pinned: the same instance types for the baseline and every candidate.
- Everything must be destroyable: no prevent_destroy, no deletion protection, no provisioners, no import blocks.
- Never put secrets, keys or real customer data in a file; seed data is synthetic or anonymised.
- Keep it minimal and cheap. Prefer one fewer machine to one more.

## When you are done
End your final message with one line, exactly:
${ANSWER_MARKER} {"scope":"none|single|full","reason":"one or two plain sentences on why this much infrastructure"}
`;
}

/** The follow-up message for the revise loop. */
export function buildReviseBrief(message: string): string {
  return `The user asked for a change to the environment you drafted:

"${message.trim()}"

Update the files in place, keeping every rule from the first brief (you may change only the environment: block of ratchet.yaml and files under ${DEFAULT_ENVIRONMENT_INFRA_DIR}/). If the request cannot be done inside the base module (for example a managed database, which the isolated network cannot host), keep the closest thing that works and say so in your reason. End your final message with the ${ANSWER_MARKER} line again.`;
}
