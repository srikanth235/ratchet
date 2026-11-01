# ---- Optional egress proxy for agent runners (default OFF) ----
#
# Why a single squid instance and not AWS Network Firewall: Network Firewall
# costs roughly $0.395/h per endpoint (~$290/month) plus $0.065/GB processed;
# one t4g.nano running squid costs ~$3/month (+ ~$3.65 public IPv4, ~$0.65 EBS).
# The trade is availability (one instance, no failover: if it dies, runners
# lose model-API access until it is replaced) and no TLS inspection - the
# allow-list matches the CONNECT hostname, which is what we need and nothing more.
#
# The proxy is only reachable from the agent subnet and is invisible to the
# reaper (it carries no ratchet:opt tag). It has no instance profile.

locals {
  aws_proxy_domains = [
    "ec2.${var.region}.amazonaws.com",
    "sts.${var.region}.amazonaws.com",
    "s3.${var.region}.amazonaws.com",
    "${aws_s3_bucket.artifacts.bucket}.s3.${var.region}.amazonaws.com",
  ]
  proxy_allowed_domains = distinct(concat(var.egress_proxy_domains, local.aws_proxy_domains, var.egress_proxy_extra_domains))
}

data "aws_ssm_parameter" "al2023_arm64" {
  count = var.enable_egress_proxy ? 1 : 0
  name  = "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64"
}

resource "aws_security_group" "egress_proxy" {
  count = var.enable_egress_proxy ? 1 : 0

  name_prefix            = "${local.name_prefix}-egress-proxy-"
  description            = "Ratchet egress proxy: 3128 from the agent subnet, 443 out"
  vpc_id                 = aws_vpc.sandbox.id
  revoke_rules_on_delete = true

  # By CIDR to avoid a dependency cycle with the agent_runner group.
  ingress {
    description = "Proxy port from the agent subnet"
    from_port   = local.proxy_port
    to_port     = local.proxy_port
    protocol    = "tcp"
    cidr_blocks = [local.agent_subnet_cidr]
  }

  egress {
    description = "HTTPS to the allow-listed upstreams (squid enforces the domains)"
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

resource "aws_instance" "egress_proxy" {
  count = var.enable_egress_proxy ? 1 : 0

  ami                         = nonsensitive(data.aws_ssm_parameter.al2023_arm64[0].value)
  instance_type               = var.egress_proxy_instance_type
  subnet_id                   = aws_subnet.proxy[0].id
  private_ip                  = local.proxy_private_ip
  vpc_security_group_ids      = [aws_security_group.egress_proxy[0].id]
  associate_public_ip_address = true
  user_data_replace_on_change = true

  user_data = templatefile("${path.module}/templates/proxy_user_data.sh.tftpl", {
    squid_conf = templatefile("${path.module}/templates/squid.conf.tftpl", {
      agent_subnet_cidr = local.agent_subnet_cidr
      proxy_port        = tostring(local.proxy_port)
      allowed_domains   = join(" ", local.proxy_allowed_domains)
    })
  })

  metadata_options {
    http_tokens                 = "required"
    http_endpoint               = "enabled"
    http_put_response_hop_limit = 1
  }

  root_block_device {
    volume_size = 8
    encrypted   = true
  }

  tags = { Name = "${local.name_prefix}-egress-proxy" }
}
