data "aws_vpc" "selected" {
  id      = var.vpc_id
  default = var.vpc_id == null ? true : null
}

data "aws_subnets" "selected" {
  filter {
    name   = "vpc-id"
    values = [data.aws_vpc.selected.id]
  }
}

locals {
  subnet_id = coalesce(var.subnet_id, try(sort(data.aws_subnets.selected.ids)[0], null))
}

# Agent runner: no inbound at all. Egress is HTTPS (model APIs, GitHub, AWS
# endpoints) and DNS to the VPC resolver. Security groups filter by IP/port,
# not domain, so 443 is open to the internet - see contract.md "Limits IAM
# cannot express". SSH-based git is therefore unavailable; use HTTPS remotes.
resource "aws_security_group" "agent_runner" {
  name_prefix            = "${local.name_prefix}-agent-runner-"
  description            = "Ratchet agent runner: no inbound, egress 443 and DNS only"
  vpc_id                 = data.aws_vpc.selected.id
  revoke_rules_on_delete = true

  egress {
    description = "HTTPS to model APIs, GitHub and AWS endpoints"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }

  egress {
    description = "DNS (UDP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "udp"
    cidr_blocks = [data.aws_vpc.selected.cidr_block]
  }

  egress {
    description = "DNS (TCP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "tcp"
    cidr_blocks = [data.aws_vpc.selected.cidr_block]
  }

  lifecycle {
    create_before_destroy = true
  }
}

# Bench machines: load generator <-> target talk to each other (self-reference)
# and nothing else may connect in - in particular nothing from agent runners,
# which are in a different group and have no matching rule.
resource "aws_security_group" "bench" {
  name_prefix            = "${local.name_prefix}-bench-"
  description            = "Ratchet bench: intra-group traffic only, no inbound from agents"
  vpc_id                 = data.aws_vpc.selected.id
  revoke_rules_on_delete = true

  ingress {
    description = "Load generator <-> target within the bench group"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    self        = true
  }

  egress {
    description = "Load generator <-> target within the bench group"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    self        = true
  }

  egress {
    description = "HTTPS for checkout, S3 and SSM (narrow via bench_egress_cidrs)"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = var.bench_egress_cidrs
  }

  egress {
    description = "DNS (UDP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "udp"
    cidr_blocks = [data.aws_vpc.selected.cidr_block]
  }

  egress {
    description = "DNS (TCP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "tcp"
    cidr_blocks = [data.aws_vpc.selected.cidr_block]
  }

  lifecycle {
    create_before_destroy = true
  }
}
