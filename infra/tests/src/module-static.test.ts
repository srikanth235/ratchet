import { readdirSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { awsDir, readAws, templateVars } from "./render.js";

const tf = (name: string) => readAws(name);
const allTf = readdirSync(awsDir)
  .filter((f) => f.endsWith(".tf"))
  .map((f) => tf(f))
  .join("\n");

describe("aws module: files and providers", () => {
  it("pins the hashicorp/aws provider and OpenTofu version", () => {
    const v = tf("versions.tf");
    expect(v).toMatch(/source\s*=\s*"hashicorp\/aws"/);
    expect(v).toMatch(/version\s*=\s*"~> \d+\.\d+"/);
    expect(v).toMatch(/required_version\s*=\s*">= \d+\.\d+\.\d+"/);
  });
  it("is split into s3.tf, iam.tf, network.tf, variables.tf, outputs.tf, main.tf", () => {
    const files = readdirSync(awsDir);
    for (const f of ["main.tf", "s3.tf", "iam.tf", "network.tf", "variables.tf", "outputs.tf", "versions.tf"]) {
      expect(files).toContain(f);
    }
  });
});

describe("aws module: the eight section-7 resources", () => {
  it.each([
    "aws_s3_bucket\" \"artifacts",
    "aws_s3_bucket\" \"sealed",
    "aws_s3_bucket_policy\" \"sealed_deny_agents",
    "aws_iam_policy\" \"agent_boundary",
    "aws_iam_role\" \"bench_runner",
    "aws_iam_instance_profile\" \"bench_runner",
    "aws_security_group\" \"agent_runner",
    "aws_security_group\" \"bench",
  ])("declares resource %s", (decl) => {
    expect(allTf).toContain(`resource "${decl}"`);
  });
});

describe("aws module: bucket hardening", () => {
  it.each(["artifacts", "sealed"])("%s bucket blocks public access, enforces ownership and encrypts", (b) => {
    const s3 = tf("s3.tf");
    expect(s3).toContain(`resource "aws_s3_bucket_public_access_block" "${b}"`);
    expect(s3).toContain(`resource "aws_s3_bucket_server_side_encryption_configuration" "${b}"`);
    expect(s3).toContain(`resource "aws_s3_bucket_ownership_controls" "${b}"`);
  });
  it("blocks all four public-access flags (and never sets one false)", () => {
    const s3 = tf("s3.tf");
    for (const f of ["block_public_acls", "block_public_policy", "ignore_public_acls", "restrict_public_buckets"]) {
      expect(s3.match(new RegExp(`${f}\\s*=\\s*true`, "g"))).toHaveLength(2);
    }
    expect(s3).not.toMatch(/(block_public_\w+|ignore_public_acls|restrict_public_buckets)\s*=\s*false/);
  });
  it("versions the sealed bucket and has a lifecycle on artifacts", () => {
    const s3 = tf("s3.tf");
    expect(s3).toMatch(/aws_s3_bucket_versioning" "sealed"[\s\S]*?status\s*=\s*"Enabled"/);
    expect(s3).toContain('resource "aws_s3_bucket_lifecycle_configuration" "artifacts"');
  });
});

describe("aws module: network", () => {
  const net = tf("network.tf");
  const block = (name: string) => net.slice(net.indexOf(`resource "aws_security_group" "${name}"`)).split(/\nresource /)[0]!;
  it("agent runner has no ingress and only 443 + DNS egress", () => {
    const b = block("agent_runner");
    expect(b).not.toMatch(/\bingress\s*\{/);
    const ports = [...b.matchAll(/egress\s*\{[^}]*?from_port\s*=\s*(\d+)[^}]*?to_port\s*=\s*(\d+)/g)].map((m) => `${m[1]}-${m[2]}`);
    expect(ports.sort()).toEqual(["443-443", "53-53", "53-53"]);
    expect(b).not.toMatch(/protocol\s*=\s*"-1"/);
  });
  it("bench ingress is self-referencing only", () => {
    const b = block("bench");
    const ingress = [...b.matchAll(/\bingress\s*\{([^}]*)\}/g)].map((m) => m[1]!);
    expect(ingress).toHaveLength(1);
    expect(ingress[0]).toMatch(/self\s*=\s*true/);
    expect(ingress[0]).not.toMatch(/cidr_blocks|security_groups/);
  });
});

describe("aws module: template wiring", () => {
  const iam = tf("iam.tf");
  const s3 = tf("s3.tf");
  const call = (src: string, template: string) => {
    const i = src.indexOf(`policies/${template}.json.tftpl`);
    expect(i).toBeGreaterThan(-1);
    const start = src.indexOf(", {", i);
    return [...src.slice(start, src.indexOf("\n  })", start)).matchAll(/^\s+([a-z_]+)\s+=/gm)].map((m) => m[1]!).sort();
  };
  it.each([
    ["agent_boundary", iam],
    ["bench_runner_policy", iam],
    ["sealed_bucket_policy", s3],
  ])("%s: templatefile() passes exactly the variables the template uses", (name, src) => {
    expect(call(src, name)).toEqual(templateVars(readAws(`policies/${name}.json.tftpl`)));
  });
});

describe("aws module: contract surface", () => {
  it("declares every contract input", () => {
    const v = tf("variables.tf");
    for (const name of ["sandbox_id", "region", "allowed_instance_types", "max_instances_per_agent", "max_instance_lifetime_hours", "tags"]) {
      expect(v).toContain(`variable "${name}"`);
    }
  });
  it("declares every contract output", () => {
    const o = tf("outputs.tf");
    for (const name of ["artifacts_bucket", "sealed_bucket", "agent_boundary_ref", "eval_identity_ref", "network_ref"]) {
      expect(o).toContain(`output "${name}"`);
    }
    expect(o).toMatch(/subnet_id/);
    expect(o).toMatch(/security_group_ids/);
  });
  it("documents every contract name in contract.md", async () => {
    const { readFileSync } = await import("node:fs");
    const md = readFileSync(new URL("../../contract.md", import.meta.url), "utf8");
    for (const name of ["sandbox_id", "region", "allowed_instance_types", "max_instances_per_agent", "max_instance_lifetime_hours", "tags", "artifacts_bucket", "sealed_bucket", "agent_boundary_ref", "eval_identity_ref", "network_ref"]) {
      expect(md).toContain(`\`${name}\``);
    }
  });
  it("keeps the agent role path in sync between iam locals, policies and contract", () => {
    expect(tf("main.tf")).toContain('agent_role_path = "/ratchet/agent/"');
  });
});
