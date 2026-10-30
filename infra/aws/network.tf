# ---- Dedicated sandbox VPC ----
#
# Layout (all /24s carved out of var.vpc_cidr, one AZ so bench hardware, load
# generator and the SSM endpoints share a zone):
#
#   bench subnet  private, NO route to the internet. Reaches only the S3
#                 gateway endpoint and the SSM/logs interface endpoints.
#   agent subnet  agent runners (and the experiment instances they launch).
#                 Default: public subnet (internet gateway), egress 443 open.
#                 With enable_egress_proxy: private, the only way out is the
#                 allow-listing proxy - enforced by the ROUTE TABLE as well as
#                 the security group, so an agent that launches an instance
#                 with a different security group still has no route out.
#   proxy subnet  only with enable_egress_proxy: public, holds the squid proxy.
#
# A VPC of its own (instead of the account's default VPC) is what makes the
# bench lockdown real: the default VPC has a public subnet and an internet
# gateway, and any security-group mistake would silently re-open the internet.

data "aws_availability_zones" "available" {
  state = "available"
}

locals {
  az = coalesce(var.availability_zone, try(sort(data.aws_availability_zones.available.names)[0], null))

  bench_subnet_cidr = cidrsubnet(var.vpc_cidr, 8, 1)
  agent_subnet_cidr = cidrsubnet(var.vpc_cidr, 8, 2)
  proxy_subnet_cidr = cidrsubnet(var.vpc_cidr, 8, 3)
  proxy_private_ip  = cidrhost(local.proxy_subnet_cidr, 10)
  proxy_port        = 3128

  # Interface endpoints the bench pipeline needs: SSM Run Command (ssm,
  # ssmmessages, ec2messages) and CloudWatch Logs.
  interface_endpoint_services = ["ssm", "ssmmessages", "ec2messages", "logs"]
}

resource "aws_vpc" "sandbox" {
  cidr_block           = var.vpc_cidr
  enable_dns_support   = true
  enable_dns_hostnames = true # required for private DNS on the interface endpoints

  tags = { Name = "${local.name_prefix}-vpc" }
}

# A VPC always ships a default security group that allows all egress. Strip it
# so nothing launched without an explicit group inherits an open egress.
resource "aws_default_security_group" "sandbox" {
  vpc_id = aws_vpc.sandbox.id
  tags   = { Name = "${local.name_prefix}-default-deny" }
}

resource "aws_internet_gateway" "sandbox" {
  vpc_id = aws_vpc.sandbox.id
  tags   = { Name = "${local.name_prefix}-igw" }
}

resource "aws_subnet" "bench" {
  vpc_id                  = aws_vpc.sandbox.id
  cidr_block              = local.bench_subnet_cidr
  availability_zone       = local.az
  map_public_ip_on_launch = false
  tags                    = { Name = "${local.name_prefix}-bench" }
}

resource "aws_subnet" "agent" {
  vpc_id            = aws_vpc.sandbox.id
  cidr_block        = local.agent_subnet_cidr
  availability_zone = local.az
  # Without the proxy the agent subnet is public and needs a public IP to
  # reach model APIs; with it there is no route out, so no public IP.
  map_public_ip_on_launch = !var.enable_egress_proxy
  tags                    = { Name = "${local.name_prefix}-agent" }
}

resource "aws_subnet" "proxy" {
  count = var.enable_egress_proxy ? 1 : 0

  vpc_id                  = aws_vpc.sandbox.id
  cidr_block              = local.proxy_subnet_cidr
  availability_zone       = local.az
  map_public_ip_on_launch = true
  tags                    = { Name = "${local.name_prefix}-proxy" }
}

# Private: only the implicit local route plus the S3 gateway endpoint prefix list.
resource "aws_route_table" "private" {
  vpc_id = aws_vpc.sandbox.id
  tags   = { Name = "${local.name_prefix}-private" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.sandbox.id
  tags   = { Name = "${local.name_prefix}-public" }
}

resource "aws_route" "public_internet" {
  route_table_id         = aws_route_table.public.id
  destination_cidr_block = "0.0.0.0/0"
  gateway_id             = aws_internet_gateway.sandbox.id
}

resource "aws_route_table_association" "bench" {
  subnet_id      = aws_subnet.bench.id
  route_table_id = aws_route_table.private.id
}

resource "aws_route_table_association" "agent" {
  subnet_id      = aws_subnet.agent.id
  route_table_id = var.enable_egress_proxy ? aws_route_table.private.id : aws_route_table.public.id
}

resource "aws_route_table_association" "proxy" {
  count = var.enable_egress_proxy ? 1 : 0

  subnet_id      = aws_subnet.proxy[0].id
  route_table_id = aws_route_table.public.id
}

# ---- VPC endpoints ----

# S3 gateway endpoint (free). The endpoint policy only admits the two sandbox
# buckets, so even a process that somehow held credentials cannot write to an
# attacker's bucket through it. Identity policies still decide what each role
# may do inside those buckets.
resource "aws_vpc_endpoint" "s3" {
  vpc_id            = aws_vpc.sandbox.id
  service_name      = "com.amazonaws.${var.region}.s3"
  vpc_endpoint_type = "Gateway"
  route_table_ids   = [aws_route_table.private.id]
  policy = templatefile("${path.module}/policies/s3_endpoint_policy.json.tftpl", {
    artifacts_bucket_arn = aws_s3_bucket.artifacts.arn
    sealed_bucket_arn    = aws_s3_bucket.sealed.arn
  })

  tags = { Name = "${local.name_prefix}-s3" }
}

resource "aws_security_group" "endpoints" {
  name_prefix            = "${local.name_prefix}-endpoints-"
  description            = "Ratchet interface endpoints: 443 from the bench and agent subnets only"
  vpc_id                 = aws_vpc.sandbox.id
  revoke_rules_on_delete = true

  # By CIDR, not by security group: bench/agent_runner reference this group in
  # their egress rules and a reference back would be a dependency cycle.
  ingress {
    description = "HTTPS from the bench and agent subnets"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = [local.bench_subnet_cidr, local.agent_subnet_cidr]
  }

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_vpc_endpoint" "interface" {
  for_each = toset(local.interface_endpoint_services)

  vpc_id              = aws_vpc.sandbox.id
  service_name        = "com.amazonaws.${var.region}.${each.value}"
  vpc_endpoint_type   = "Interface"
  subnet_ids          = [aws_subnet.bench.id]
  security_group_ids  = [aws_security_group.endpoints.id]
  private_dns_enabled = true

  tags = { Name = "${local.name_prefix}-${each.value}" }
}

# ---- Agent runner ----
#
# No inbound at all. Egress:
#   proxy off: HTTPS to anywhere (model APIs, GitHub, AWS) + DNS. Security
#              groups filter by IP/port, not domain - see contract.md "Limits
#              IAM cannot express". SSH-based git is therefore unavailable.
#   proxy on:  only the proxy (3128), the interface endpoints and the S3
#              prefix list (443) + DNS. The proxy allow-lists domains.
resource "aws_security_group" "agent_runner" {
  name_prefix            = "${local.name_prefix}-agent-runner-"
  description            = "Ratchet agent runner: no inbound; egress via the allow-list proxy or HTTPS"
  vpc_id                 = aws_vpc.sandbox.id
  revoke_rules_on_delete = true

  dynamic "egress" {
    for_each = var.enable_egress_proxy ? [] : [1]
    content {
      description = "HTTPS to model APIs, GitHub and AWS endpoints"
      from_port   = 443
      to_port     = 443
      protocol    = "tcp"
      cidr_blocks = ["0.0.0.0/0"]
    }
  }

  dynamic "egress" {
    for_each = var.enable_egress_proxy ? [1] : []
    content {
      description     = "Allow-list egress proxy"
      from_port       = local.proxy_port
      to_port         = local.proxy_port
      protocol        = "tcp"
      security_groups = [aws_security_group.egress_proxy[0].id]
    }
  }

  dynamic "egress" {
    for_each = var.enable_egress_proxy ? [1] : []
    content {
      description     = "SSM / logs interface endpoints"
      from_port       = 443
      to_port         = 443
      protocol        = "tcp"
      security_groups = [aws_security_group.endpoints.id]
    }
  }

  dynamic "egress" {
    for_each = var.enable_egress_proxy ? [1] : []
    content {
      description     = "S3 gateway endpoint"
      from_port       = 443
      to_port         = 443
      protocol        = "tcp"
      prefix_list_ids = [aws_vpc_endpoint.s3.prefix_list_id]
    }
  }

  egress {
    description = "DNS (UDP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "udp"
    cidr_blocks = [aws_vpc.sandbox.cidr_block]
  }

  egress {
    description = "DNS (TCP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "tcp"
    cidr_blocks = [aws_vpc.sandbox.cidr_block]
  }

  lifecycle {
    create_before_destroy = true
  }
}

# ---- Bench machines ----
#
# Candidate code runs here, next to the hidden trace on confirm runs, so the
# group has NO route to the internet: egress is the group itself (load
# generator <-> target), the SSM/logs interface endpoints, the S3 gateway
# endpoint and DNS. Nothing may connect in - in particular nothing from agent
# runners, which are in a different group and have no matching rule.
resource "aws_security_group" "bench" {
  name_prefix            = "${local.name_prefix}-bench-"
  description            = "Ratchet bench: intra-group traffic and VPC endpoints only; no internet, no inbound from agents"
  vpc_id                 = aws_vpc.sandbox.id
  revoke_rules_on_delete = true

  ingress {
    description = "Load generator to target within the bench group"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    self        = true
  }

  egress {
    description = "Load generator to target within the bench group"
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    self        = true
  }

  egress {
    description     = "SSM / logs interface endpoints"
    from_port       = 443
    to_port         = 443
    protocol        = "tcp"
    security_groups = [aws_security_group.endpoints.id]
  }

  egress {
    description     = "S3 gateway endpoint (pre-staged candidate, results, confirm trace)"
    from_port       = 443
    to_port         = 443
    protocol        = "tcp"
    prefix_list_ids = [aws_vpc_endpoint.s3.prefix_list_id]
  }

  # NOTE: security groups do not filter traffic to the Amazon DNS resolver, so
  # these two rules document intent but do not stop DNS tunnelling; see
  # docs/real-env/bench.md "Residual risks" (Route 53 Resolver DNS Firewall).
  egress {
    description = "DNS (UDP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "udp"
    cidr_blocks = [aws_vpc.sandbox.cidr_block]
  }

  egress {
    description = "DNS (TCP) to the VPC resolver"
    from_port   = 53
    to_port     = 53
    protocol    = "tcp"
    cidr_blocks = [aws_vpc.sandbox.cidr_block]
  }

  lifecycle {
    create_before_destroy = true
  }
}
