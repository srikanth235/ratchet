import { describe, expect, it } from "vitest";
import { evaluate } from "./iam-sim.js";
import { fixture, renderPolicy } from "./render.js";

const sealed = fixture.sealed_bucket_arn;
const acct = fixture.account_id;
const agentRole = `arn:aws:iam::${acct}:role/ratchet/agent/ratchet-opt1-claude-1`;
const benchRole = `arn:aws:iam::${acct}:role/ratchet/bench/ratchet-demo-bench-runner`;
const adminRole = `arn:aws:iam::${acct}:role/Admin`;

describe("sealed bucket policy", () => {
  const policy = renderPolicy("sealed_bucket_policy");
  const decide = (principalArn: string, context: Record<string, string> = {}, action = "s3:GetObject") =>
    evaluate(policy, { action, resource: `${sealed}/trace.json`, context: { "aws:PrincipalArn": principalArn, "aws:SecureTransport": "true", ...context } });

  it("is deny-only with Principal * (grants come from identity policies)", () => {
    for (const s of policy.Statement) {
      expect(s.Effect).toBe("Deny");
      expect(s.Principal).toBe("*");
    }
  });

  it.each(["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket", "s3:PutBucketPolicy"])(
    "denies agent roles by path for %s",
    (action) => {
      expect(evaluate(policy, { action, resource: sealed, context: { "aws:PrincipalArn": agentRole, "aws:SecureTransport": "true" } })).toBe("ExplicitDeny");
    },
  );
  it("denies anything tagged ratchet:agent even if outside the agent path", () => {
    expect(decide(adminRole, { "aws:PrincipalTag/ratchet:agent": "claude-1" })).toBe("ExplicitDeny");
  });
  it("does not deny the bench runner or an untagged admin (their access comes from identity policy)", () => {
    expect(decide(benchRole)).toBe("ImplicitDeny");
    expect(decide(adminRole)).toBe("ImplicitDeny");
  });
  it("denies non-TLS access for everyone", () => {
    expect(decide(benchRole, { "aws:SecureTransport": "false" })).toBe("ExplicitDeny");
  });
  it("matches the agent role path the module exposes", () => {
    expect(fixture.agent_role_arn_pattern).toContain(":role/ratchet/agent/");
    expect(fixture.agent_role_arn_pattern).not.toContain("/ratchet/bench/");
  });
});

describe("bench runner policy", () => {
  const policy = renderPolicy("bench_runner_policy");
  const decide = (action: string, resource: string) => evaluate(policy, { action, resource });

  it("can read the sealed bucket", () => {
    expect(decide("s3:GetObject", `${sealed}/trace.json`)).toBe("Allow");
    expect(decide("s3:ListBucket", sealed)).toBe("Allow");
  });
  it("cannot write or delete in the sealed bucket", () => {
    expect(decide("s3:PutObject", `${sealed}/trace.json`)).toBe("ImplicitDeny");
    expect(decide("s3:DeleteObject", `${sealed}/trace.json`)).toBe("ImplicitDeny");
  });
  it("writes results only under bench-results/ and cannot read or overwrite agent prefixes", () => {
    const a = fixture.artifacts_bucket_arn;
    expect(decide("s3:PutObject", `${a}/bench-results/opt1/r.json`)).toBe("Allow");
    expect(decide("s3:PutObject", `${a}/artifacts/opt1/claude-1/x`)).toBe("ImplicitDeny");
    expect(decide("s3:GetObject", `${a}/artifacts/opt1/claude-1/x`)).toBe("ImplicitDeny");
  });
  it("has no IAM/organizations/sts and no wildcard actions", () => {
    for (const s of policy.Statement) {
      for (const a of [s.Action].flat() as string[]) expect(a).not.toMatch(/^(\*|iam:|organizations:|sts:|ec2:Run|ec2:Terminate)/);
    }
  });
  it("reads only the bench secrets", () => {
    const p = (path: string) => `arn:aws:ssm:${fixture.region}:${acct}:parameter${path}`;
    expect(decide("ssm:GetParameter", p("/ratchet/bench/github-token"))).toBe("Allow");
    expect(decide("ssm:GetParameter", p("/ratchet/opt1/claude-1/anthropic-key"))).toBe("ImplicitDeny");
  });
});
