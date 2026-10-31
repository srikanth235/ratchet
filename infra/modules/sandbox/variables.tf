# The base module a drafted environment is built on ("ratchet/sandbox").
#
# A setup agent drafts an environment as a few lines of OpenTofu that call THIS module; it brings everything the sandbox
# contract (infra/contract.md) needs on the measuring side, so a draft cannot weaken it:
#   - placement: every machine goes into the sandbox's private bench subnet, in the bench security group (no route to the
#     internet, intra-group traffic and the VPC endpoints only), never with a public address;
#   - identity: the evaluation-only instance profile (no access to the sealed store, results or holdout);
#   - hardening: IMDSv2 required, encrypted volumes, terminate on shutdown, no termination protection;
#   - tags: every machine and volume carries the sandbox / environment tags the budget and teardown receipt rely on.
#
# `ratchet` is injected by Ratchet from the sandbox's contract outputs (a draft passes it through as `ratchet = var.ratchet`
# and never sets it). `machines` is the only thing a draft decides.

variable "ratchet" {
  description = "The sandbox this environment lives in: the standard sandbox module's contract outputs, injected by Ratchet."
  type = object({
    sandbox_id                 = string
    region                     = string
    vpc_id                     = string
    bench_subnet_id            = string
    bench_security_group_id    = string
    artifacts_bucket           = string
    eval_instance_profile_name = string
    # Content hash of the approved environment (tag `ratchet:env`).
    environment_hash = string
    # The bench machine image. Empty: the newest image tagged ratchet:image=bench (and ratchet:image_version=image_version).
    machine_image_id = optional(string, "")
    image_version    = optional(string, "")
    tags             = optional(map(string), {})
  })
}

variable "machines" {
  description = "The machines of the environment, by name (lowercase letters, digits, hyphens). A literal map: the hardware must be readable before anything is created."
  type = map(object({
    role           = string
    instance_type  = string
    label          = optional(string, "")
    detail         = optional(string, "")
    user_data      = optional(string, "")
    root_volume_gb = optional(number, 30)
    data_volume_gb = optional(number, 0)
  }))

  validation {
    condition     = length(var.machines) >= 1 && length(var.machines) <= 8
    error_message = "an environment has 1 to 8 machines."
  }

  validation {
    condition     = alltrue([for k, m in var.machines : can(regex("^[a-z][a-z0-9-]{0,23}$", k))])
    error_message = "machine names must be 1-24 characters: lowercase letters, digits and hyphens, starting with a letter."
  }

  validation {
    condition     = alltrue([for k, m in var.machines : contains(["app", "database", "cache", "load", "evaluator", "other"], m.role)])
    error_message = "role must be one of app, database, cache, load, evaluator, other."
  }

  validation {
    condition     = alltrue([for k, m in var.machines : can(regex("^[a-z][a-z0-9]*\\.[a-z0-9]+$", m.instance_type))])
    error_message = "instance_type must be an exact instance type such as c7i.2xlarge."
  }

  validation {
    # Burstable instances change speed with CPU credits: not for the machines the metric is measured on.
    condition     = alltrue([for k, m in var.machines : !(contains(["app", "load", "evaluator"], m.role) && can(regex("^t[0-9]", m.instance_type)))])
    error_message = "app, load and evaluator machines must be fixed-performance instance types (c7i, m7i, r7i, ...), not burstable t-types."
  }

  validation {
    condition     = alltrue([for k, m in var.machines : m.root_volume_gb >= 8 && m.root_volume_gb <= 500 && m.data_volume_gb >= 0 && m.data_volume_gb <= 1000])
    error_message = "root_volume_gb must be 8-500 and data_volume_gb 0-1000."
  }
}
