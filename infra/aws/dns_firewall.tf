# ---- DNS Firewall for the sandbox VPC ----
#
# Why: security groups do not filter traffic to the Amazon DNS resolver
# (VPC base address + 2), so a process in the bench subnet could tunnel data out
# by looking up <data>.attacker.example: the resolver forwards the query to the
# attacker's name server. Route 53 Resolver DNS Firewall sits in the resolver and
# answers NXDOMAIN for every name that is not on the allow-list, so the query
# never leaves AWS.
#
# Scope: a rule group is associated per VPC, not per subnet, and this module has
# ONE VPC shared by bench machines, agent runners and the optional proxy.
# Putting the bench in a second VPC would mean duplicating the four interface
# endpoints (about $29/month per copy), the S3 endpoint and every security-group
# and boundary reference. Instead one allow-list covers both consumers:
#
#   bench    only the names of the endpoints it uses (S3, ssm, ssmmessages,
#            ec2messages, logs) and the VPC's internal names
#   agents   the same AWS names plus the agent domain allow-list that already
#            exists for the egress proxy (egress_proxy_domains and
#            egress_proxy_extra_domains) plus dns_firewall_extra_domains
#
# Consequence for agent runners: a name that is not on the list does not
# resolve, with or without the proxy. That is a domain allow-list one layer
# below squid, effective even when the proxy is off, but it is a change from
# "HTTPS to anywhere": add what the runner image needs (package registries) to
# dns_firewall_extra_domains. Connecting to an IP literal is not stopped here;
# that is the job of the security groups, the route tables and the proxy.
#
# Limits: the firewall decides on the queried NAME. An allowed name (say
# <bucket>.s3.<region>.amazonaws.com) still resolves, and queries for allowed
# AWS zones are answered by AWS, not by a name server the sender controls, so
# they carry nothing to an attacker. Fail closed: if the firewall cannot decide,
# the query is refused (firewall_fail_open = DISABLED).

locals {
  # Bench path: one name per interface endpoint (ssm, ssmmessages, ec2messages,
  # logs) and the S3 names the gateway endpoint serves (bucket-style
  # <bucket>.s3.<region> and path-style s3.<region>).
  dns_bench_domains = concat(
    [for s in local.interface_endpoint_services : "${s}.${var.region}.amazonaws.com"],
    ["s3.${var.region}.amazonaws.com", "*.s3.${var.region}.amazonaws.com"],
    # Old CLIs address us-east-1 buckets through the global name.
    var.region == "us-east-1" ? ["s3.amazonaws.com", "*.s3.amazonaws.com"] : [],
  )

  # Names an instance resolves for itself (hostname lookups by sudo and
  # friends, reverse lookups of VPC addresses). Private to the VPC: nobody
  # outside can register a PTR record for a 10.x address.
  vpc_octets = split(".", cidrhost(var.vpc_cidr, 0))
  dns_internal_domains = [
    "ec2.internal",
    "*.ec2.internal",
    "${var.region}.compute.internal",
    "*.${var.region}.compute.internal",
    "*.${local.vpc_octets[1]}.${local.vpc_octets[0]}.in-addr.arpa",
  ]

  # squid dstdomain syntax (".github.com" = the domain and its subdomains) ->
  # DNS Firewall syntax (exact names; "*." for subdomains).
  dns_agent_domains = flatten([
    for d in local.proxy_allowed_domains :
    startswith(d, ".") ? [trimprefix(d, "."), "*${d}"] : [d]
  ])

  # `dnf install squid` on the proxy instance.
  dns_proxy_bootstrap_domains = var.enable_egress_proxy ? ["cdn.amazonlinux.com", "*.s3.dualstack.${var.region}.amazonaws.com"] : []

  dns_allowed_domains = sort(distinct(concat(
    local.dns_bench_domains,
    local.dns_internal_domains,
    local.dns_agent_domains,
    local.dns_proxy_bootstrap_domains,
    var.dns_firewall_extra_domains,
  )))
}

resource "aws_route53_resolver_firewall_domain_list" "allowed" {
  count = var.enable_dns_firewall ? 1 : 0

  name    = "${local.name_prefix}-dns-allowed"
  domains = local.dns_allowed_domains
}

# "*" matches every name. Evaluated after the allow rule (higher priority number).
resource "aws_route53_resolver_firewall_domain_list" "everything" {
  count = var.enable_dns_firewall ? 1 : 0

  name    = "${local.name_prefix}-dns-everything"
  domains = ["*"]
}

resource "aws_route53_resolver_firewall_rule_group" "sandbox" {
  count = var.enable_dns_firewall ? 1 : 0

  name = "${local.name_prefix}-dns-firewall"
}

resource "aws_route53_resolver_firewall_rule" "allow_listed" {
  count = var.enable_dns_firewall ? 1 : 0

  name                    = "allow-listed"
  action                  = "ALLOW"
  firewall_domain_list_id = aws_route53_resolver_firewall_domain_list.allowed[0].id
  firewall_rule_group_id  = aws_route53_resolver_firewall_rule_group.sandbox[0].id
  priority                = 100
}

resource "aws_route53_resolver_firewall_rule" "block_everything_else" {
  count = var.enable_dns_firewall ? 1 : 0

  name                    = "block-everything-else"
  action                  = "BLOCK"
  block_response          = "NXDOMAIN"
  firewall_domain_list_id = aws_route53_resolver_firewall_domain_list.everything[0].id
  firewall_rule_group_id  = aws_route53_resolver_firewall_rule_group.sandbox[0].id
  priority                = 200
}

resource "aws_route53_resolver_firewall_rule_group_association" "sandbox" {
  count = var.enable_dns_firewall ? 1 : 0

  name                   = "${local.name_prefix}-dns-firewall"
  firewall_rule_group_id = aws_route53_resolver_firewall_rule_group.sandbox[0].id
  vpc_id                 = aws_vpc.sandbox.id
  priority               = 101
  # DISABLED so `tofu destroy` can remove it; the sandbox account is ephemeral.
  mutation_protection = "DISABLED"

  # Rules first: an empty group associated with the VPC would briefly allow everything.
  depends_on = [
    aws_route53_resolver_firewall_rule.allow_listed,
    aws_route53_resolver_firewall_rule.block_everything_else,
  ]
}

# Explicit fail-closed (also the AWS default): when the firewall cannot decide,
# the resolver refuses the query instead of answering it.
resource "aws_route53_resolver_firewall_config" "sandbox" {
  count = var.enable_dns_firewall ? 1 : 0

  resource_id        = aws_vpc.sandbox.id
  firewall_fail_open = "DISABLED"
}
