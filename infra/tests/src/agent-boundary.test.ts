import { describe, expect, it } from "vitest";
import { evaluate, type Request } from "./iam-sim.js";
import { fixture, renderPolicy, type PolicyDocument } from "./render.js";

const boundary = renderPolicy("agent_boundary");
const acct = fixture.account_id;
const region = fixture.region;
const ec2 = (kind: string, id = "i-0123456789abcdef0") => `arn:aws:ec2:${region}:${acct}:${kind}/${id}`;
const artifacts = fixture.artifacts_bucket_arn;
const sealed = fixture.sealed_bucket_arn;

const tags = { "aws:PrincipalTag/ratchet:opt": "opt1", "aws:PrincipalTag/ratchet:agent": "claude-1" };
const as = (req: Request, principal: Record<string, string> = tags): Request => ({
  ...req,
  context: { ...principal, ...req.context },
});
const decide = (req: Request, principal?: Record<string, string>) => evaluate(boundary, as(req, principal));

const goodRun = {
  action: "ec2:RunInstances",
  resource: ec2("instance", "*"),
  context: {
    "ec2:InstanceType": "t3.medium",
    "ec2:MetadataHttpTokens": "required",
    "aws:RequestTag/ratchet:opt": "opt1",
    "aws:RequestTag/ratchet:agent": "claude-1",
  },
};

describe("agent boundary: structure", () => {
  it("is a well-formed policy with unique Sids and no NotAction/NotResource", () => {
    expect(boundary.Version).toBe("2012-10-17");
    const sids = boundary.Statement.map((s) => s.Sid);
    expect(new Set(sids).size).toBe(sids.length);
    for (const s of boundary.Statement) {
      expect(s.Sid).toBeTruthy();
      expect(s).not.toHaveProperty("NotAction");
      expect(s).not.toHaveProperty("NotResource");
      expect(s).not.toHaveProperty("Principal");
    }
  });

  it("never allows control-plane services, and only wildcard-resource-allows read-only/channel actions", () => {
    const allows = boundary.Statement.filter((s) => s.Effect === "Allow");
    const wildcardAllows = allows.filter((s) => [s.Resource].flat().includes("*"));
    const actions = wildcardAllows.flatMap((s) => [s.Action].flat() as string[]);
    for (const a of actions) {
      expect(a).toMatch(/^(ec2:Describe\*|ec2:GetConsoleOutput|ssm:UpdateInstanceInformation|ssmmessages:|ec2messages:)/);
    }
    for (const s of allows) {
      for (const a of [s.Action].flat() as string[]) {
        expect(a).not.toMatch(/^(iam|organizations|sts|billing|ce|budgets|account|\*)(:|$)/);
      }
    }
  });
});

describe("agent boundary: sealed bucket", () => {
  const sealedRequests = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket", "s3:GetBucketPolicy", "s3:PutBucketPolicy", "s3:*"];
  it.each(sealedRequests)("explicitly denies %s on the sealed bucket and its objects", (action) => {
    expect(decide({ action, resource: sealed })).toBe("ExplicitDeny");
    expect(decide({ action, resource: `${sealed}/traces/hidden.jsonl` })).toBe("ExplicitDeny");
  });

  it("denies sealed access even when the request would otherwise match an artifacts allow", () => {
    expect(decide({ action: "s3:GetObject", resource: `${sealed}/artifacts/opt1/claude-1/x` })).toBe("ExplicitDeny");
  });
});

describe("agent boundary: artifacts storage", () => {
  const own = `${artifacts}/artifacts/opt1/claude-1/events.jsonl`;
  it("allows get/put under its own prefix", () => {
    expect(decide({ action: "s3:GetObject", resource: own })).toBe("Allow");
    expect(decide({ action: "s3:PutObject", resource: own })).toBe("Allow");
  });
  it.each([
    ["other agent", `${artifacts}/artifacts/opt1/codex-1/events.jsonl`],
    ["other optimization", `${artifacts}/artifacts/opt2/claude-1/events.jsonl`],
    ["bucket root", `${artifacts}/events.jsonl`],
    ["bench results", `${artifacts}/bench-results/opt1/r.json`],
    ["staged bench inputs", `${artifacts}/bench-inputs/opt1/job/code.tgz`],
    ["prefix-collision sibling", `${artifacts}/artifacts/opt1/claude-10/events.jsonl`],
  ])("does not allow objects in %s", (_n, resource) => {
    expect(decide({ action: "s3:GetObject", resource })).toBe("ImplicitDeny");
    expect(decide({ action: "s3:PutObject", resource })).toBe("ImplicitDeny");
  });
  it("does not allow deleting objects or touching bucket configuration", () => {
    expect(decide({ action: "s3:DeleteObject", resource: own })).toBe("ImplicitDeny");
    expect(decide({ action: "s3:PutBucketPolicy", resource: artifacts })).toBe("ImplicitDeny");
  });
  it("allows ListBucket only with its own prefix", () => {
    const list = (prefix?: string): Request => ({
      action: "s3:ListBucket",
      resource: artifacts,
      context: prefix === undefined ? {} : { "s3:prefix": prefix },
    });
    expect(decide(list("artifacts/opt1/claude-1/"))).toBe("Allow");
    expect(decide(list("artifacts/opt1/claude-1/sub/"))).toBe("Allow");
    expect(decide(list(""))).toBe("ImplicitDeny");
    expect(decide(list())).toBe("ImplicitDeny");
    expect(decide(list("artifacts/"))).toBe("ImplicitDeny");
    expect(decide(list("artifacts/opt1/"))).toBe("ImplicitDeny");
    expect(decide(list("artifacts/opt2/claude-1/"))).toBe("ImplicitDeny");
  });
});

describe("agent boundary: secrets (SSM)", () => {
  const param = (path: string) => `arn:aws:ssm:${region}:${acct}:parameter${path}`;
  it("allows GetParameter only under its own /ratchet/<opt>/<agent>/", () => {
    expect(decide({ action: "ssm:GetParameter", resource: param("/ratchet/opt1/claude-1/anthropic-key") })).toBe("Allow");
    expect(decide({ action: "ssm:GetParameter", resource: param("/ratchet/opt1/codex-1/openai-key") })).toBe("ImplicitDeny");
    expect(decide({ action: "ssm:GetParameter", resource: param("/ratchet/opt2/claude-1/anthropic-key") })).toBe("ImplicitDeny");
    expect(decide({ action: "ssm:GetParameter", resource: param("/ratchet/bench/github-token") })).toBe("ImplicitDeny");
    expect(decide({ action: "ssm:GetParameter", resource: param("/other/secret") })).toBe("ImplicitDeny");
  });
  it("does not allow writing or listing parameters", () => {
    const r = param("/ratchet/opt1/claude-1/x");
    expect(decide({ action: "ssm:PutParameter", resource: r })).toBe("ImplicitDeny");
    expect(decide({ action: "ssm:DeleteParameter", resource: r })).toBe("ImplicitDeny");
    expect(decide({ action: "ssm:GetParametersByPath", resource: param("/ratchet/opt1") })).toBe("ImplicitDeny");
  });
});

describe("agent boundary: launching instances", () => {
  it("allows RunInstances with matching request tags, an allow-listed type and IMDSv2", () => {
    expect(decide(goodRun)).toBe("Allow");
    expect(decide({ ...goodRun, context: { ...goodRun.context, "ec2:InstanceType": "c7i.large" } })).toBe("Allow");
  });

  it("requires the ratchet:opt request tag", () => {
    const { ["aws:RequestTag/ratchet:opt"]: _drop, ...rest } = goodRun.context;
    expect(decide({ ...goodRun, context: rest })).toBe("ImplicitDeny");
  });
  it("requires the ratchet:agent request tag", () => {
    const { ["aws:RequestTag/ratchet:agent"]: _drop, ...rest } = goodRun.context;
    expect(decide({ ...goodRun, context: rest })).toBe("ImplicitDeny");
  });
  it("rejects request tags that name a different optimization or agent", () => {
    expect(decide({ ...goodRun, context: { ...goodRun.context, "aws:RequestTag/ratchet:opt": "opt2" } })).toBe("ImplicitDeny");
    expect(decide({ ...goodRun, context: { ...goodRun.context, "aws:RequestTag/ratchet:agent": "codex-1" } })).toBe("ImplicitDeny");
  });

  it.each(["p4d.24xlarge", "t3.medium.evil", "c7i.large ", "T3.MEDIUM", "m5.large"])(
    "explicitly denies instance type %j",
    (type) => {
      expect(decide({ ...goodRun, context: { ...goodRun.context, "ec2:InstanceType": type } })).toBe("ExplicitDeny");
    },
  );
  it("allows only types from the allow-list rendered into the policy", () => {
    const body = JSON.stringify(boundary);
    for (const t of fixture.allowedTypes) expect(body).toContain(t);
    expect(body).not.toContain("p4d");
  });

  it("explicitly denies instances without IMDSv2", () => {
    expect(decide({ ...goodRun, context: { ...goodRun.context, "ec2:MetadataHttpTokens": "optional" } })).toBe("ExplicitDeny");
    const { ["ec2:MetadataHttpTokens"]: _drop, ...rest } = goodRun.context;
    expect(decide({ ...goodRun, context: rest })).toBe("ExplicitDeny");
  });

  it("allows security groups and ENIs only inside the sandbox VPC", () => {
    for (const kind of ["security-group", "network-interface"]) {
      const base = { action: "ec2:RunInstances", resource: ec2(kind) };
      expect(decide({ ...base, context: { "ec2:Vpc": fixture.vpc_arn } })).toBe("Allow");
      expect(decide({ ...base, context: { "ec2:Vpc": "arn:aws:ec2:us-east-1:123456789012:vpc/vpc-other" } })).toBe("ImplicitDeny");
      expect(decide(base)).toBe("ImplicitDeny");
    }
    expect(decide({ action: "ec2:RunInstances", resource: `arn:aws:ec2:${region}::image/ami-0abc` })).toBe("Allow");
  });

  it("allows only the agent subnet - never the bench or proxy subnet", () => {
    const inVpc = { "ec2:Vpc": fixture.vpc_arn };
    expect(decide({ action: "ec2:RunInstances", resource: fixture.agent_subnet_arn, context: inVpc })).toBe("Allow");
    expect(decide({ action: "ec2:RunInstances", resource: fixture.bench_subnet_arn, context: inVpc })).toBe("ImplicitDeny");
    expect(decide({ action: "ec2:RunInstances", resource: ec2("subnet", "subnet-0proxy"), context: inVpc })).toBe("ImplicitDeny");
    expect(decide({ action: "ec2:RunInstances", resource: fixture.agent_subnet_arn })).toBe("ImplicitDeny");
  });

  it.each([fixture.bench_sg_arn, fixture.endpoints_sg_arn, fixture.proxy_sg_arn])(
    "explicitly denies launching into the internal security group %s",
    (sg) => {
      expect(decide({ action: "ec2:RunInstances", resource: sg, context: { "ec2:Vpc": fixture.vpc_arn } })).toBe("ExplicitDeny");
    },
  );
  it("still allows the agent runner security group", () => {
    expect(
      decide({ action: "ec2:RunInstances", resource: ec2("security-group", "sg-0runner"), context: { "ec2:Vpc": fixture.vpc_arn } }),
    ).toBe("Allow");
  });

  it("allows CreateTags only as part of RunInstances", () => {
    const base = { action: "ec2:CreateTags", resource: ec2("instance") };
    expect(decide({ ...base, context: { "ec2:CreateAction": "RunInstances" } })).toBe("Allow");
    expect(decide(base)).toBe("ImplicitDeny");
    expect(decide({ ...base, context: { "ec2:CreateAction": "CreateVolume" } })).toBe("ImplicitDeny");
    expect(decide({ action: "ec2:DeleteTags", resource: ec2("instance") })).toBe("ImplicitDeny");
  });

  it("denies launching with an instance profile or changing hardening", () => {
    for (const action of ["iam:PassRole", "ec2:AssociateIamInstanceProfile", "ec2:ReplaceIamInstanceProfileAssociation", "ec2:ModifyInstanceMetadataOptions", "ec2:ModifyInstanceAttribute"]) {
      expect(decide({ action, resource: "*" })).toBe("ExplicitDeny");
    }
  });
});

describe("agent boundary: instance lifecycle", () => {
  const own = { "ec2:ResourceTag/ratchet:opt": "opt1", "ec2:ResourceTag/ratchet:agent": "claude-1" };
  it.each(["ec2:StartInstances", "ec2:StopInstances", "ec2:RebootInstances", "ec2:TerminateInstances"])(
    "allows %s only on instances tagged with the caller's own opt and agent",
    (action) => {
      const resource = ec2("instance");
      expect(decide({ action, resource, context: own })).toBe("Allow");
      expect(decide({ action, resource, context: { ...own, "ec2:ResourceTag/ratchet:opt": "opt2" } })).toBe("ImplicitDeny");
      expect(decide({ action, resource, context: { ...own, "ec2:ResourceTag/ratchet:agent": "codex-1" } })).toBe("ImplicitDeny");
      expect(decide({ action, resource, context: {} })).toBe("ImplicitDeny");
    },
  );
});

describe("agent boundary: control plane", () => {
  it.each([
    "iam:CreateRole", "iam:PassRole", "iam:DeleteRolePermissionsBoundary", "iam:PutRolePolicy", "iam:AttachRolePolicy",
    "iam:DeleteUserPermissionsBoundary", "iam:CreatePolicyVersion",
    "organizations:DescribeOrganization", "account:GetContactInformation",
    "billing:GetBillingData", "ce:GetCostAndUsage", "aws-portal:ViewBilling", "budgets:ViewBudget",
    "sts:AssumeRole", "sts:TagSession",
  ])("explicitly denies %s", (action) => {
    expect(decide({ action, resource: "*" })).toBe("ExplicitDeny");
    expect(decide({ action, resource: `arn:aws:iam::${acct}:role/ratchet/bench/ratchet-demo-bench-runner` })).toBe("ExplicitDeny");
  });

  it("does not allow unrelated services", () => {
    for (const action of ["s3:CreateBucket", "lambda:InvokeFunction", "ec2:CreateSecurityGroup", "ec2:AuthorizeSecurityGroupIngress", "ec2:CreateVpc", "kms:Decrypt", "ec2:RunInstances"]) {
      expect(decide({ action, resource: "*" })).not.toBe("Allow");
    }
  });
});

describe("agent boundary: fail closed", () => {
  const untagged: Record<string, string>[] = [
    {},
    { "aws:PrincipalTag/ratchet:opt": "opt1" },
    { "aws:PrincipalTag/ratchet:agent": "claude-1" },
  ];
  it.each(untagged.map((t, i) => [i, t] as const))("denies everything for a principal with tags #%i", (_i, principal) => {
    expect(decide(goodRun, principal)).toBe("ExplicitDeny");
    expect(decide({ action: "s3:GetObject", resource: `${artifacts}/artifacts/opt1/claude-1/x` }, principal)).toBe("ExplicitDeny");
    expect(decide({ action: "ssm:GetParameter", resource: `arn:aws:ssm:${region}:${acct}:parameter/ratchet/opt1/claude-1/k` }, principal)).toBe("ExplicitDeny");
  });
});

describe("agent boundary: cross-agent isolation", () => {
  it("a different agent only gets its own prefix", () => {
    const other = { "aws:PrincipalTag/ratchet:opt": "opt1", "aws:PrincipalTag/ratchet:agent": "codex-1" };
    const req = { action: "s3:GetObject", resource: `${artifacts}/artifacts/opt1/claude-1/events.jsonl` };
    expect(decide(req, other)).toBe("ImplicitDeny");
    expect(decide({ ...req, resource: `${artifacts}/artifacts/opt1/codex-1/events.jsonl` }, other)).toBe("Allow");
  });
});

// Pure guard so a refactor can't quietly drop the explicit-deny block.
describe("agent boundary: required explicit denies are present", () => {
  const denies = (doc: PolicyDocument) => doc.Statement.filter((s) => s.Effect === "Deny").map((s) => s.Sid);
  it("lists the expected Deny statements", () => {
    expect(denies(boundary)).toEqual(
      expect.arrayContaining([
        "DenyPrincipalWithoutOptTag",
        "DenyPrincipalWithoutAgentTag",
        "DenySealedBucket",
        "DenyControlPlaneAndBilling",
        "DenyRunInstancesOtherTypes",
        "DenyRunInstancesWithoutImdsV2",
        "DenyInstanceHardeningChanges",
      ]),
    );
  });
});
