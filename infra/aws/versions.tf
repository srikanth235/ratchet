terraform {
  required_version = ">= 1.8.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.100"
    }
  }
}

provider "aws" {
  region = var.region

  # Applied to every taggable resource. Per-agent / per-optimization tags
  # (ratchet:opt, ratchet:agent) are set at runtime by the app, never here.
  default_tags {
    tags = merge(var.tags, {
      "ratchet:sandbox" = var.sandbox_id
      "ratchet:managed" = "opentofu"
    })
  }
}
