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

variable "vpc_cidr" {
  description = "CIDR of the dedicated sandbox VPC created by the module (the account's default VPC is never used). /16 or /20; /24 subnets are carved out of it."
  type        = string
  default     = "10.42.0.0/16"

  validation {
    condition     = can(cidrnetmask(var.vpc_cidr)) && tonumber(split("/", var.vpc_cidr)[1]) <= 20 && tonumber(split("/", var.vpc_cidr)[1]) >= 16
    error_message = "vpc_cidr must be a valid IPv4 CIDR with a prefix length between /16 and /20."
  }
}

variable "availability_zone" {
  description = "AZ for every subnet (bench hardware, load generator and endpoints share it). Null = first available AZ of the region."
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

variable "sealed_confirm_prefix" {
  description = "Key prefix in the sealed bucket that the confirm runner may read (the hidden holdout traces). Ends with \"/\"; lowercase letters, digits, \"-\", \"_\" and \"/\" only - it is substituted into an IAM policy, so no wildcards."
  type        = string
  default     = "holdout/"

  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9_-]*(/[a-z0-9][a-z0-9_-]*)*/$", var.sealed_confirm_prefix))
    error_message = "sealed_confirm_prefix must look like \"holdout/\" or \"traces/v1/\": lowercase letters, digits, \"-\" and \"_\" segments, each followed by \"/\"; no wildcards, \"..\" or empty segments."
  }
}

# ---- Agent runner egress allow-list proxy (default off) ----

variable "enable_egress_proxy" {
  description = "Put agent runners behind a squid allow-list proxy (domains only) and remove their route to the internet. Default off: runners then have HTTPS egress to anywhere. Costs about $7/month (t4g.nano + public IPv4 + EBS); see docs/real-env/aws.md."
  type        = bool
  default     = false
}

variable "egress_proxy_instance_type" {
  description = "Instance type of the proxy (arm64 / Graviton, because the AMI looked up is Amazon Linux 2023 arm64)."
  type        = string
  default     = "t4g.nano"

  validation {
    condition     = can(regex("^(t4g|c7g|m7g|c6g|m6g)\\.[a-z0-9]+$", var.egress_proxy_instance_type))
    error_message = "egress_proxy_instance_type must be a Graviton (arm64) type such as t4g.nano."
  }
}

variable "egress_proxy_domains" {
  description = "Domains agent runners may reach through the proxy (squid dstdomain syntax: a leading dot also matches subdomains)."
  type        = list(string)
  default     = ["api.anthropic.com", "api.openai.com", ".github.com", ".githubusercontent.com"]

  validation {
    condition     = alltrue([for d in var.egress_proxy_domains : can(regex("^\\.?[a-z0-9][a-z0-9.-]*\\.[a-z]{2,}$", d))])
    error_message = "egress_proxy_domains entries must be lowercase domain names, optionally with one leading dot; no wildcards, spaces or ports."
  }
}

variable "egress_proxy_extra_domains" {
  description = "Additional allowed domains, e.g. a package registry (registry.npmjs.org) the runner image needs."
  type        = list(string)
  default     = []

  validation {
    condition     = alltrue([for d in var.egress_proxy_extra_domains : can(regex("^\\.?[a-z0-9][a-z0-9.-]*\\.[a-z]{2,}$", d))])
    error_message = "egress_proxy_extra_domains entries must be lowercase domain names, optionally with one leading dot."
  }
}
