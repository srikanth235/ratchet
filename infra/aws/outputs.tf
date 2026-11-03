# Contract outputs (infra/contract.md).

output "artifacts_bucket" {
  description = "Agent read/write bucket; one prefix per agent: artifacts/<opt>/<agent>/."
  value       = aws_s3_bucket.artifacts.bucket
}

output "sealed_bucket" {
  description = "Hidden-trace bucket; confirm runner only (holdout prefix)."
  value       = aws_s3_bucket.sealed.bucket
}

output "agent_boundary_ref" {
  description = "Permission boundary ARN to attach to every per-agent role."
  value       = aws_iam_policy.agent_boundary.arn
}

output "eval_identity_ref" {
  description = "Bench runner identity for baseline/bench phases. NO sealed access."
  value = {
    role_arn              = aws_iam_role.bench_runner.arn
    role_name             = aws_iam_role.bench_runner.name
    instance_profile_arn  = aws_iam_instance_profile.bench_runner.arn
    instance_profile_name = aws_iam_instance_profile.bench_runner.name
  }
}

output "confirm_identity_ref" {
  description = "Confirm runner identity: the only one that can read the sealed bucket (holdout prefix). Use only for purpose=confirm."
  value = {
    role_arn              = aws_iam_role.confirm_runner.arn
    role_name             = aws_iam_role.confirm_runner.name
    instance_profile_arn  = aws_iam_instance_profile.confirm_runner.arn
    instance_profile_name = aws_iam_instance_profile.confirm_runner.name
  }
}

output "deps_identity_ref" {
  description = "Dependency-snapshot builder identity (purpose=deps): runs in the agent subnet (egress), reads only the staged base tree, writes only bench-deps/. NO sealed access. Never runs candidate code."
  value = {
    role_arn              = aws_iam_role.deps_runner.arn
    role_name             = aws_iam_role.deps_runner.name
    instance_profile_arn  = aws_iam_instance_profile.deps_runner.arn
    instance_profile_name = aws_iam_instance_profile.deps_runner.name
  }
}

output "sealed_confirm_prefix" {
  description = "Key prefix in sealed_bucket that holds the confirm traces."
  value       = var.sealed_confirm_prefix
}

output "network_ref" {
  description = "Where to launch machines. Bench instances go in bench_subnet_id (no internet route)."
  value = {
    vpc_id          = aws_vpc.sandbox.id
    subnet_id       = aws_subnet.agent.id
    bench_subnet_id = aws_subnet.bench.id
    security_group_ids = {
      agent_runner = aws_security_group.agent_runner.id
      bench        = aws_security_group.bench.id
    }
    # Null unless enable_egress_proxy; runners get it as HTTPS_PROXY.
    egress_proxy_url = var.enable_egress_proxy ? "http://${local.proxy_private_ip}:${local.proxy_port}" : null
  }
}

output "agent_role_path" {
  description = "IAM path under which per-agent roles MUST be created."
  value       = local.agent_role_path
}

output "limits" {
  description = "Limits IAM cannot enforce; enforced by the app reaper."
  value = {
    max_instances_per_agent     = var.max_instances_per_agent
    max_instance_lifetime_hours = var.max_instance_lifetime_hours
  }
}
