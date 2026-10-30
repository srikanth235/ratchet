# Ratchet AWS sandbox module (v0). Implements infra/contract.md.
#
# Layout: s3.tf (artifacts + sealed buckets), iam.tf (agent boundary, bench and
# confirm runners), network.tf (VPC, endpoints, security groups), proxy.tf
# (optional egress proxy), outputs.tf (contract outputs).
#
# Created at RUNTIME by the app, not here (see infra/README.md): per-agent IAM
# roles under local.agent_role_path with the boundary attached, runner and
# experiment instances, SSM SecureString secrets.

data "aws_caller_identity" "current" {}
data "aws_partition" "current" {}

locals {
  name_prefix = "ratchet-${var.sandbox_id}"

  # Per-agent roles MUST be created under this path. The sealed bucket policy
  # and the boundary key off it; the bench runner lives under a different path.
  agent_role_path = "/ratchet/agent/"
  bench_role_path = "/ratchet/bench/"

  partition  = data.aws_partition.current.partition
  account_id = data.aws_caller_identity.current.account_id
}
