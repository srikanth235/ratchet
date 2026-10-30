# ---- Agent permission boundary ----
#
# A permission boundary only CAPS what a role may do; the per-agent role the app
# creates at runtime still needs an identity policy, and effective permissions
# are the intersection. The policy document lives in
# policies/agent_boundary.json.tftpl so it is testable without AWS (infra/tests).
#
# Enforced here (IAM):
#   - ec2:RunInstances only with request tags ratchet:opt / ratchet:agent equal
#     to the caller's own principal tags, allow-listed ec2:InstanceType, IMDSv2
#     required, only into the AGENT subnet (never the bench or proxy subnet),
#     never with the bench / endpoint / proxy security groups, no instance
#     profile (iam:* denied, so no iam:PassRole)
#   - start/stop/reboot/terminate only on instances whose tags match the caller
#   - S3 only under artifacts/<opt>/<agent>/; ssm:GetParameter only under
#     /ratchet/<opt>/<agent>/
#   - explicit deny: sealed bucket, iam:*, organizations:*, billing/cost,
#     sts:AssumeRole*; deny everything for principals missing the tags
#
# NOT enforceable by IAM (enforced by the app's reaper / cost guard):
#   - max_instances_per_agent: no counting condition exists for RunInstances
#   - max_instance_lifetime_hours: no TTL condition exists
#   - InstanceInitiatedShutdownBehavior=terminate: no condition key exists for
#     this RunInstances parameter; the runner image sets it and the reaper
#     terminates stopped agent instances
#   - spend budget
resource "aws_iam_policy" "agent_boundary" {
  name        = "${local.name_prefix}-agent-boundary"
  path        = "/ratchet/"
  description = "Permission boundary for every Ratchet agent role (sandbox ${var.sandbox_id})"
  policy = templatefile("${path.module}/policies/agent_boundary.json.tftpl", {
    partition                   = local.partition
    account_id                  = local.account_id
    region                      = var.region
    artifacts_bucket_arn        = aws_s3_bucket.artifacts.arn
    sealed_bucket_arn           = aws_s3_bucket.sealed.arn
    vpc_arn                     = aws_vpc.sandbox.arn
    agent_subnet_arn            = aws_subnet.agent.arn
    allowed_instance_types_json = jsonencode(var.allowed_instance_types)
    # Groups agents must never launch into: bench (would join the bench group's
    # self-referencing ingress), the endpoint group and, if enabled, the proxy.
    denied_security_group_arns_json = jsonencode(concat(
      [aws_security_group.bench.arn, aws_security_group.endpoints.arn],
      aws_security_group.egress_proxy[*].arn,
    ))
  })
}

# ---- Trusted bench identities ----
#
# Two identities, because candidate code runs on the instance that holds them:
#
#   bench_runner    baseline / bench phases. Reads staged candidates
#                   (bench-inputs/), writes bench-results/. NO sealed access -
#                   and the sealed bucket policy denies it explicitly - so a
#                   candidate that steals these credentials finds nothing hidden.
#   confirm_runner  confirm phase only. Same, plus read of the holdout prefix
#                   (var.sealed_confirm_prefix) of the sealed bucket.
#
# Both sit outside the agent path, so the sealed bucket's agent deny does not
# apply and the agent boundary does not cover them. Agents cannot assume them
# (trust is ec2.amazonaws.com only) nor pass them to an instance (iam:* denied).
# The app picks the profile from BenchLaunchSpec.purpose.

data "aws_iam_policy_document" "bench_trust" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["ec2.amazonaws.com"]
    }
    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [local.account_id]
    }
  }
}

resource "aws_iam_role" "bench_runner" {
  name                 = "${local.name_prefix}-bench-runner"
  path                 = local.bench_role_path
  description          = "Trusted bench runner for baseline/bench phases: no sealed bucket access"
  assume_role_policy   = data.aws_iam_policy_document.bench_trust.json
  max_session_duration = 3600
}

resource "aws_iam_instance_profile" "bench_runner" {
  name = "${local.name_prefix}-bench-runner"
  path = local.bench_role_path
  role = aws_iam_role.bench_runner.name
}

resource "aws_iam_role_policy" "bench_runner" {
  name = "bench-runner"
  role = aws_iam_role.bench_runner.id
  policy = templatefile("${path.module}/policies/bench_runner_policy.json.tftpl", {
    artifacts_bucket_arn = aws_s3_bucket.artifacts.arn
  })
}

resource "aws_iam_role" "confirm_runner" {
  name                 = "${local.name_prefix}-confirm-runner"
  path                 = local.bench_role_path
  description          = "Trusted bench runner for confirm runs only: reads the holdout prefix of the sealed bucket"
  assume_role_policy   = data.aws_iam_policy_document.bench_trust.json
  max_session_duration = 3600
}

resource "aws_iam_instance_profile" "confirm_runner" {
  name = "${local.name_prefix}-confirm-runner"
  path = local.bench_role_path
  role = aws_iam_role.confirm_runner.name
}

resource "aws_iam_role_policy" "confirm_runner" {
  name = "confirm-runner"
  role = aws_iam_role.confirm_runner.id
  policy = templatefile("${path.module}/policies/confirm_runner_policy.json.tftpl", {
    artifacts_bucket_arn  = aws_s3_bucket.artifacts.arn
    sealed_bucket_arn     = aws_s3_bucket.sealed.arn
    sealed_confirm_prefix = var.sealed_confirm_prefix
  })
}
