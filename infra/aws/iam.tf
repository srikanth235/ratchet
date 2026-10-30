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
#     required, subnet/SG/ENI only inside the sandbox VPC, no instance profile
#     (iam:* denied, so no iam:PassRole)
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
    vpc_arn                     = data.aws_vpc.selected.arn
    allowed_instance_types_json = jsonencode(var.allowed_instance_types)
  })
}

# ---- Trusted bench runner ----
# Outside the agent path, so the sealed bucket's agent deny does not apply and
# the agent boundary does not cover it. Agents cannot assume it (trust is
# ec2.amazonaws.com only) nor pass it to an instance (iam:* denied).

data "aws_iam_policy_document" "bench_runner_trust" {
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
  description          = "Trusted re-bench runner with access to the sealed bucket"
  assume_role_policy   = data.aws_iam_policy_document.bench_runner_trust.json
  max_session_duration = 3600
}

resource "aws_iam_instance_profile" "bench_runner" {
  name = "${local.name_prefix}-bench-runner"
  path = local.bench_role_path
  role = aws_iam_role.bench_runner.name
}

# Not one of the 8 headline resources, but the role is useless without a policy.
resource "aws_iam_role_policy" "bench_runner" {
  name = "bench-runner"
  role = aws_iam_role.bench_runner.id
  policy = templatefile("${path.module}/policies/bench_runner_policy.json.tftpl", {
    partition            = local.partition
    account_id           = local.account_id
    region               = var.region
    artifacts_bucket_arn = aws_s3_bucket.artifacts.arn
    sealed_bucket_arn    = aws_s3_bucket.sealed.arn
  })
}
