output "tags" {
  description = "Tags every resource of the environment must carry (budget, teardown receipt)."
  value       = local.tags
}

output "artifacts_bucket" {
  description = "The sandbox's artifacts bucket: seed data goes under the env/ prefix."
  value       = var.ratchet.artifacts_bucket
}

output "environment" {
  description = "What the environment created, for Ratchet (private addresses only)."
  value = {
    hash = var.ratchet.environment_hash
    machines = {
      for k, m in aws_instance.machine : k => {
        id         = m.id
        role       = var.machines[k].role
        private_ip = m.private_ip
      }
    }
  }
}
