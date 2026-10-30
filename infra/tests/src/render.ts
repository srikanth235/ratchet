import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const awsDir = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "aws");

export type PolicyVars = Record<string, string>;

/**
 * Renders a `.json.tftpl` the way OpenTofu's templatefile() would for the
 * subset we use: `${name}` interpolation and `$${` as an escape for a literal
 * `${` (IAM policy variables). Anything else (`%{` directives, expressions,
 * unknown names) throws so templates cannot silently depend on more.
 */
export function renderTemplate(template: string, vars: PolicyVars): string {
  if (template.includes("%{")) throw new Error("template directives (%{) are not supported");
  const escape = "\u0000ESC\u0000";
  const rendered = template
    .replaceAll("$${", `${escape}{`)
    .replace(/\$\{([^}]*)\}/g, (_m, name: string) => {
      if (!/^[a-z_]+$/.test(name)) throw new Error(`unsupported template expression: \${${name}}`);
      const value = vars[name];
      if (value === undefined) throw new Error(`missing template variable: ${name}`);
      return value;
    })
    .replaceAll(escape, "$");
  return rendered;
}

/** Names of the `${var}` interpolations a template uses (escapes excluded). */
export function templateVars(template: string): string[] {
  const stripped = template.replaceAll("$${", "");
  return [...new Set([...stripped.matchAll(/\$\{([^}]*)\}/g)].map((m) => m[1]!))].sort();
}

export function readAws(relative: string): string {
  return readFileSync(join(awsDir, relative), "utf8");
}

export const fixture = {
  partition: "aws",
  account_id: "123456789012",
  region: "us-east-1",
  sandboxId: "demo",
  artifacts_bucket_arn: "arn:aws:s3:::ratchet-demo-artifacts-123456789012",
  sealed_bucket_arn: "arn:aws:s3:::ratchet-demo-sealed-123456789012",
  vpc_arn: "arn:aws:ec2:us-east-1:123456789012:vpc/vpc-0abc",
  allowedTypes: ["t3.medium", "c7i.large"],
  agent_role_arn_pattern: "arn:aws:iam::123456789012:role/ratchet/agent/*",
};

export function fixtureVars(): PolicyVars {
  return {
    partition: fixture.partition,
    account_id: fixture.account_id,
    region: fixture.region,
    artifacts_bucket_arn: fixture.artifacts_bucket_arn,
    sealed_bucket_arn: fixture.sealed_bucket_arn,
    vpc_arn: fixture.vpc_arn,
    allowed_instance_types_json: JSON.stringify(fixture.allowedTypes),
    agent_role_arn_pattern: fixture.agent_role_arn_pattern,
  };
}

export function renderPolicy(name: string, vars: PolicyVars = fixtureVars()): PolicyDocument {
  return JSON.parse(renderTemplate(readAws(`policies/${name}.json.tftpl`), vars)) as PolicyDocument;
}

export type Condition = Record<string, Record<string, string | string[]>>;

export interface Statement {
  Sid?: string;
  Effect: "Allow" | "Deny";
  Principal?: unknown;
  Action?: string | string[];
  NotAction?: unknown;
  Resource?: string | string[];
  NotResource?: unknown;
  Condition?: Condition;
}

export interface PolicyDocument {
  Version: string;
  Statement: Statement[];
}
