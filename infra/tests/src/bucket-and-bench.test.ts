import { describe, expect, it } from "vitest";
import { evaluate } from "./iam-sim.js";
import { fixture, renderPolicy } from "./render.js";

const sealed = fixture.sealed_bucket_arn;
const acct = fixture.account_id;
const agentRole = `arn:aws:iam::${acct}:role/ratchet/agent/ratchet-opt1-claude-1`;
const benchRole = fixture.bench_runner_role_arn;
const confirmRole = fixture.confirm_runner_role_arn;
const depsRole = fixture.deps_runner_role_arn;
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
  it("explicitly denies the bench runner (no sealed access in baseline/bench phases)", () => {
    for (const action of ["s3:GetObject", "s3:ListBucket", "s3:PutObject", "s3:DeleteObject"]) {
      expect(decide(benchRole, {}, action)).toBe("ExplicitDeny");
    }
  });
  it("explicitly denies the dependency-snapshot builder too (it has egress, so it must hold nothing hidden)", () => {
    for (const action of ["s3:GetObject", "s3:ListBucket", "s3:PutObject", "s3:DeleteObject"]) {
      expect(decide(depsRole, {}, action)).toBe("ExplicitDeny");
    }
  });
  it("does not deny the confirm runner or an untagged admin (their access comes from identity policy)", () => {
    expect(decide(confirmRole)).toBe("ImplicitDeny");
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

const ssmPath = (path: string) => `arn:aws:ssm:${fixture.region}:${acct}:parameter${path}`;
const channel = ["ssm:UpdateInstanceInformation", "ssmmessages:OpenDataChannel", "ec2messages:GetMessages"];

describe.each([
  ["bench_runner_policy", false],
  ["confirm_runner_policy", true],
] as const)("%s", (name, canReadSealed) => {
  const policy = renderPolicy(name);
  const decide = (action: string, resource: string, context: Record<string, string> = {}) => evaluate(policy, { action, resource, context });
  const a = fixture.artifacts_bucket_arn;

  it("reads staged candidates only from bench-inputs/", () => {
    expect(decide("s3:GetObject", `${a}/bench-inputs/opt1/job1/code.tgz`)).toBe("Allow");
    expect(decide("s3:GetObject", `${a}/artifacts/opt1/claude-1/x`)).toBe("ImplicitDeny");
    expect(decide("s3:GetObject", `${a}/bench-results/opt1/r.json`)).toBe("ImplicitDeny");
    expect(decide("s3:PutObject", `${a}/bench-inputs/opt1/job1/code.tgz`)).toBe("ImplicitDeny");
  });
  it("reads dependency snapshots from bench-deps/ but can never write them", () => {
    expect(decide("s3:GetObject", `${a}/bench-deps/pnpm-abc/1234.tar.zst`)).toBe("Allow");
    expect(decide("s3:GetObject", `${a}/bench-deps/pnpm-abc.json`)).toBe("Allow");
    expect(decide("s3:PutObject", `${a}/bench-deps/pnpm-abc/1234.tar.zst`)).toBe("ImplicitDeny");
    expect(decide("s3:DeleteObject", `${a}/bench-deps/pnpm-abc.json`)).toBe("ImplicitDeny");
  });
  it("writes results only under bench-results/ and cannot read or overwrite agent prefixes", () => {
    expect(decide("s3:PutObject", `${a}/bench-results/opt1/r.json`)).toBe("Allow");
    expect(decide("s3:PutObject", `${a}/artifacts/opt1/claude-1/x`)).toBe("ImplicitDeny");
    expect(decide("s3:DeleteObject", `${a}/bench-results/opt1/r.json`)).toBe("ImplicitDeny");
  });
  it("cannot write or delete in the sealed bucket", () => {
    expect(decide("s3:PutObject", `${sealed}/holdout/trace.json`)).toBe("ImplicitDeny");
    expect(decide("s3:DeleteObject", `${sealed}/holdout/trace.json`)).toBe("ImplicitDeny");
  });
  it("has no IAM/organizations/sts, no wildcard actions and no secrets access", () => {
    for (const s of policy.Statement) {
      for (const act of [s.Action].flat() as string[]) expect(act).not.toMatch(/^(\*|iam:|organizations:|sts:|ec2:Run|ec2:Terminate|ssm:Get)/);
    }
    expect(decide("ssm:GetParameter", ssmPath("/ratchet/bench/github-token"))).toBe("ImplicitDeny");
    expect(decide("ssm:GetParameter", ssmPath("/ratchet/opt1/claude-1/anthropic-key"))).toBe("ImplicitDeny");
  });
  it("keeps the SSM managed-instance channel", () => {
    for (const act of channel) expect(decide(act, "*")).toBe("Allow");
  });

  if (canReadSealed) {
    it("reads only the holdout prefix of the sealed bucket", () => {
      expect(decide("s3:GetObject", `${sealed}/holdout/trace.json`)).toBe("Allow");
      expect(decide("s3:GetObject", `${sealed}/holdout/sub/dir/trace.json`)).toBe("Allow");
      expect(decide("s3:GetObject", `${sealed}/other/trace.json`)).toBe("ImplicitDeny");
      expect(decide("s3:GetObject", `${sealed}/holdout-extra/trace.json`)).toBe("ImplicitDeny");
      expect(decide("s3:GetObjectVersion", `${sealed}/holdout/trace.json`)).toBe("ImplicitDeny");
    });
    it("lists the sealed bucket only under the holdout prefix", () => {
      expect(decide("s3:ListBucket", sealed, { "s3:prefix": "holdout/" })).toBe("Allow");
      expect(decide("s3:ListBucket", sealed, { "s3:prefix": "holdout/sub/" })).toBe("Allow");
      expect(decide("s3:ListBucket", sealed, { "s3:prefix": "" })).toBe("ImplicitDeny");
      expect(decide("s3:ListBucket", sealed, { "s3:prefix": "other/" })).toBe("ImplicitDeny");
      expect(decide("s3:ListBucket", sealed)).toBe("ImplicitDeny");
    });
  } else {
    it("has no sealed bucket access whatsoever", () => {
      for (const action of ["s3:GetObject", "s3:ListBucket", "s3:PutObject", "s3:GetObjectVersion"]) {
        expect(decide(action, `${sealed}/holdout/trace.json`)).toBe("ImplicitDeny");
        expect(decide(action, sealed, { "s3:prefix": "holdout/x" })).toBe("ImplicitDeny");
      }
      expect(JSON.stringify(policy)).not.toContain(sealed);
    });
  }
});

describe("deps_runner_policy (dependency-snapshot builder: has egress, runs no candidate code)", () => {
  const policy = renderPolicy("deps_runner_policy");
  const decide = (action: string, resource: string, context: Record<string, string> = {}) => evaluate(policy, { action, resource, context });
  const a = fixture.artifacts_bucket_arn;

  it("writes only under bench-deps/", () => {
    expect(decide("s3:PutObject", `${a}/bench-deps/pnpm-abc/1234.tar.zst`)).toBe("Allow");
    expect(decide("s3:PutObject", `${a}/bench-deps/pnpm-abc.json`)).toBe("Allow");
    expect(decide("s3:AbortMultipartUpload", `${a}/bench-deps/pnpm-abc/1234.tar.zst`)).toBe("Allow");
    for (const key of ["bench-inputs/opt1/job1/deps-base.tgz", "bench-inputs/opt1/job1/code.tgz", "bench-results/opt1/r.json", "artifacts/opt1/claude-1/x", "engine/opt1/teardown.json", "x"]) {
      expect(decide("s3:PutObject", `${a}/${key}`), key).toBe("ImplicitDeny");
    }
    expect(decide("s3:DeleteObject", `${a}/bench-deps/pnpm-abc.json`)).toBe("ImplicitDeny");
  });
  it("reads only the staged base tree, never the candidate archive, other snapshots, results or agent prefixes", () => {
    expect(decide("s3:GetObject", `${a}/bench-inputs/opt1/0f5e2c6a-aaaa/deps-base.tgz`)).toBe("Allow");
    for (const key of ["bench-inputs/opt1/job1/code.tgz", "bench-inputs/opt1/job1/bench.tgz", "bench-deps/pnpm-abc.json", "bench-results/opt1/r.json", "artifacts/opt1/claude-1/x"]) {
      expect(decide("s3:GetObject", `${a}/${key}`), key).toBe("ImplicitDeny");
    }
  });
  it("cannot list the bucket", () => {
    expect(decide("s3:ListBucket", a)).toBe("ImplicitDeny");
  });
  it("has no sealed bucket access whatsoever", () => {
    for (const action of ["s3:GetObject", "s3:ListBucket", "s3:PutObject", "s3:GetObjectVersion"]) {
      expect(decide(action, `${sealed}/holdout/trace.json`)).toBe("ImplicitDeny");
      expect(decide(action, sealed, { "s3:prefix": "holdout/x" })).toBe("ImplicitDeny");
    }
    expect(JSON.stringify(policy)).not.toContain("sealed");
  });
  it("has no IAM/organizations/sts/ec2, no wildcard actions and no secrets access", () => {
    for (const s of policy.Statement) {
      for (const act of [s.Action].flat() as string[]) expect(act).not.toMatch(/^(\*|iam:|organizations:|sts:|ec2:Run|ec2:Terminate|ssm:Get|s3:\*)/);
    }
    expect(decide("ssm:GetParameter", `arn:aws:ssm:${fixture.region}:${acct}:parameter/ratchet/opt1/claude-1/anthropic-key`)).toBe("ImplicitDeny");
  });
  it("keeps the SSM managed-instance channel", () => {
    for (const act of channel) expect(decide(act, "*")).toBe("Allow");
  });
});

describe("bench / confirm policies stay in lock-step", () => {
  const bench = renderPolicy("bench_runner_policy");
  const confirm = renderPolicy("confirm_runner_policy");
  it("confirm = bench + exactly the two holdout statements", () => {
    const sids = (p: typeof bench) => p.Statement.map((s) => s.Sid).sort();
    expect(sids(confirm)).toEqual([...sids(bench), "ListHoldoutTraces", "ReadHoldoutTraces"].sort());
    for (const s of bench.Statement) expect(confirm.Statement).toContainEqual(s);
  });
});

describe("S3 gateway endpoint policy", () => {
  const policy = renderPolicy("s3_endpoint_policy");
  const decide = (action: string, resource: string) => evaluate(policy, { action, resource });
  it("admits the two sandbox buckets and nothing else", () => {
    expect(decide("s3:GetObject", `${fixture.artifacts_bucket_arn}/bench-inputs/x`)).toBe("Allow");
    expect(decide("s3:PutObject", `${fixture.artifacts_bucket_arn}/bench-results/x`)).toBe("Allow");
    expect(decide("s3:GetObject", `${sealed}/holdout/t.json`)).toBe("Allow");
    expect(decide("s3:PutObject", `${fixture.artifacts_bucket_arn}/bench-deps/pnpm-abc.json`)).toBe("Allow");
    expect(decide("s3:PutObject", "arn:aws:s3:::attacker-bucket/loot")).toBe("ImplicitDeny");
    expect(decide("s3:GetObject", "arn:aws:s3:::ratchet-demo-artifacts-123456789012-evil/x")).toBe("ImplicitDeny");
    expect(decide("s3:GetObject", "arn:aws:s3:::al2023-repos-us-east-1/x")).toBe("ImplicitDeny");
  });
  it("is allow-only and never grants bucket-management actions", () => {
    for (const s of policy.Statement) {
      expect(s.Effect).toBe("Allow");
      for (const act of [s.Action].flat() as string[]) expect(act).not.toMatch(/\*|Bucket(Policy|Acl)|Delete/);
    }
  });
});
