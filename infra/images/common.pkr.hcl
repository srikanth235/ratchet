# Shared by runner.pkr.hcl and bench.pkr.hcl (Packer loads every *.pkr.hcl in this directory).
#
#   packer init  infra/images
#   packer build -only='runner.*' -var image_version=... infra/images     # or use build.sh
#
# See README.md for the full procedure, the IAM the builder needs and how to hand the AMI ids
# to live mode (runnerImageRef / benchImageRef).

packer {
  required_version = ">= 1.10.0"
  required_plugins {
    amazon = {
      source  = "github.com/hashicorp/amazon"
      version = "= 1.8.3"
    }
  }
}

variable "image_version" {
  type        = string
  description = "Version stamped into the AMI name and the ratchet:image_version tag, e.g. 20261030-1a2b3c4. build.sh derives it from the date and the commit."
  validation {
    condition     = can(regex("^[A-Za-z0-9._-]{1,40}$", var.image_version))
    error_message = "The image_version may contain only letters, digits, '.', '_' and '-' (max 40 characters)."
  }
}

variable "source_commit" {
  type        = string
  default     = "unknown"
  description = "Commit of this repository the ratchet-runner tarballs were packed from (ratchet:source_commit tag)."
}

variable "region" {
  type        = string
  default     = "us-east-1"
  description = "Region the AMIs are built in. AMIs are regional: build in the sandbox's region (and account)."
}

variable "arch" {
  type        = string
  default     = "x86_64"
  description = "Instance architecture of the image: x86_64 (c7i, t3, ...) or arm64 (Graviton). Must match the instance types the app launches from it."
  validation {
    condition     = contains(["x86_64", "arm64"], var.arch)
    error_message = "The arch must be x86_64 or arm64."
  }
}

variable "instance_type" {
  type        = string
  default     = ""
  description = "Builder instance type. Empty: t3.medium for x86_64, t4g.medium for arm64."
}

variable "source_ami" {
  type        = string
  default     = ""
  description = "Pin the Amazon Linux 2023 base image (ami-...). Empty: the newest AL2023 AMI of the architecture. Pin it, with dnf_args, for reproducible builds."
}

variable "dnf_args" {
  type        = string
  default     = ""
  description = "One extra dnf argument, e.g. --releasever=2023.9.20261012 to pin the AL2023 repository snapshot (see README, Reproducibility). No spaces."
  validation {
    condition     = !can(regex("[[:space:]]", var.dnf_args))
    error_message = "The dnf_args must be a single argument without spaces."
  }
}

variable "vpc_id" {
  type        = string
  default     = ""
  description = "VPC of the builder instance. Empty: the region's default VPC. The sandbox's bench subnet has no internet and cannot be used."
}

variable "subnet_id" {
  type        = string
  default     = ""
  description = "Subnet of the builder instance (needs outbound internet for dnf, npm and the vendor downloads). Empty: Packer picks one in the default VPC."
}

variable "encrypt_boot" {
  type        = bool
  default     = true
  description = "Encrypt the AMI's snapshots. With the default aws/ebs key the AMI can only be launched from the account that built it, which is the intended setup (build in the sandbox account)."
}

variable "kms_key_id" {
  type        = string
  default     = ""
  description = "Customer managed KMS key for the snapshots (needed to share an encrypted AMI with other accounts). Empty: the aws/ebs key."
}

variable "ami_users" {
  type        = list(string)
  default     = []
  description = "Account ids the AMI is shared with (build centrally, launch in the sandbox account). Needs a customer managed kms_key_id when encrypt_boot is true."
}

variable "runner_volume_gb" {
  type        = number
  default     = 30
  description = "Root volume of the runner image (repository checkouts, node_modules and build outputs live here)."
}

variable "bench_volume_gb" {
  type        = number
  default     = 16
  description = "Root volume of the bench image."
}

variable "host_toolchain" {
  type        = bool
  default     = false
  description = "Bench image only: also install node, pnpm, k6 and hyperfine on the host, for repos without environment.image (the legacy no-container path). The default image is generic: the toolchain is the repo's container. The result is tagged ratchet:image=bench-host and is never found by the tag lookup."
}

variable "extra_tags" {
  type        = map(string)
  default     = {}
  description = "Extra tags for the AMI and its snapshots. The ratchet:* keys set by the template cannot be overridden."
}

locals {
  builder_type = var.instance_type != "" ? var.instance_type : (var.arch == "arm64" ? "t4g.medium" : "t3.medium")

  # Tags every AMI and snapshot carries. `ratchet:image` (runner|bench) is added per image.
  common_tags = merge(var.extra_tags, {
    "ratchet:image_version" = var.image_version
    "ratchet:source_commit" = var.source_commit
    "ratchet:arch"          = var.arch
    "ratchet:managed"       = "packer"
  })

  # Where the provisioning scripts land on the builder (the file provisioners upload to here).
  remote_dir = "/tmp/ratchet-image"
}
