# Contract outputs (infra/contract.md).

output "artifacts_bucket" {
  description = "Agent read/write bucket; one prefix per agent: artifacts/<opt>/<agent>/."
  value       = aws_s3_bucket.artifacts.bucket
}

output "sealed_bucket" {
  description = "Hidden-trace bucket; bench runner only."
  value       = aws_s3_bucket.sealed.bucket
}

output "agent_boundary_ref" {
  description = "Permission boundary ARN to attach to every per-agent role."
  value       = aws_iam_policy.agent_boundary.arn
}

output "eval_identity_ref" {
  description = "Bench runner identity."
  value = {
    role_arn              = aws_iam_role.bench_runner.arn
    role_name             = aws_iam_role.bench_runner.name
    instance_profile_arn  = aws_iam_instance_profile.bench_runner.arn
    instance_profile_name = aws_iam_instance_profile.bench_runner.name
  }
}

output "network_ref" {
  description = "Where to launch machines."
  value = {
    vpc_id    = data.aws_vpc.selected.id
    subnet_id = local.subnet_id
    security_group_ids = {
      agent_runner = aws_security_group.agent_runner.id
      bench        = aws_security_group.bench.id
    }
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
