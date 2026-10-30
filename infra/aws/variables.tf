# ---- Sandbox contract inputs (see infra/contract.md) ----

variable "sandbox_id" {
  description = "Stable id of the sandbox; prefixes every named resource."
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{1,31}$", var.sandbox_id))
    error_message = "sandbox_id must be 2-32 chars of lowercase letters, digits and hyphens, starting with a letter or digit."
  }
}

variable "region" {
  description = "AWS region for the sandbox."
  type        = string
}

variable "allowed_instance_types" {
  description = "Instance types agents may launch for experiments."
  type        = list(string)
  default     = ["t3.medium", "t3.large", "c7i.large", "c7i.xlarge"]

  validation {
    condition     = length(var.allowed_instance_types) > 0 && alltrue([for t in var.allowed_instance_types : can(regex("^[a-z0-9]+\\.[a-z0-9]+$", t))])
    error_message = "allowed_instance_types must be a non-empty list of exact instance types (no wildcards), e.g. \"c7i.large\"."
  }
}

variable "max_instances_per_agent" {
  description = "Max concurrent experiment instances per agent. NOT enforceable by IAM; enforced by the app reaper. Exposed via the `limits` output."
  type        = number
  default     = 4

  validation {
    condition     = var.max_instances_per_agent >= 1 && floor(var.max_instances_per_agent) == var.max_instances_per_agent
    error_message = "max_instances_per_agent must be a positive integer."
  }
}

variable "max_instance_lifetime_hours" {
  description = "Max lifetime of an agent-launched instance. NOT enforceable by IAM; enforced by the app reaper. Exposed via the `limits` output."
  type        = number
  default     = 2

  validation {
    condition     = var.max_instance_lifetime_hours > 0
    error_message = "max_instance_lifetime_hours must be positive."
  }
}

variable "tags" {
  description = "Extra tags for every resource. ratchet:* keys are reserved."
  type        = map(string)
  default     = {}

  validation {
    condition     = alltrue([for k in keys(var.tags) : !startswith(lower(k), "ratchet:")])
    error_message = "Tag keys starting with \"ratchet:\" are reserved."
  }
}

# ---- AWS-specific optional inputs ----

variable "vpc_id" {
  description = "VPC to launch into. Null = the account's default VPC."
  type        = string
  default     = null
}

variable "subnet_id" {
  description = "Subnet to launch into. Null = first subnet (sorted by id) of the VPC."
  type        = string
  default     = null
}

variable "artifacts_retention_days" {
  description = "Days after which artifacts objects expire."
  type        = number
  default     = 30
}

variable "force_destroy" {
  description = "Allow `tofu destroy` to delete non-empty buckets (all versions). The sandbox account is dedicated and ephemeral, so true by default."
  type        = bool
  default     = true
}

variable "bench_egress_cidrs" {
  description = "CIDRs the bench security group may reach on 443 (repo checkout, S3, SSM). WARNING: candidate code runs on the bench machine next to the sealed data; narrow this (VPC endpoints / prefix lists) to limit exfiltration."
  type        = list(string)
  default     = ["0.0.0.0/0"]
}
