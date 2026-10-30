# DNS Firewall and bench-inputs lifecycle, with the same mocked AWS provider as
# sandbox.tftest.hcl - no account or credentials needed.
# Run: tofu init -backend=false && tofu test

mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = { account_id = "123456789012" }
  }
  mock_data "aws_partition" {
    defaults = { partition = "aws" }
  }
  mock_data "aws_availability_zones" {
    defaults = { names = ["us-east-1a"] }
  }
  mock_data "aws_ssm_parameter" {
    defaults = { value = "ami-0123456789abcdef0" }
  }
  mock_data "aws_iam_policy_document" {
    defaults = {
      json = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"ec2.amazonaws.com\"},\"Action\":\"sts:AssumeRole\"}]}"
    }
  }
  mock_resource "aws_s3_bucket" {
    defaults = { arn = "arn:aws:s3:::mock-bucket" }
  }
  mock_resource "aws_iam_role" {
    defaults = { arn = "arn:aws:iam::123456789012:role/ratchet/bench/mock" }
  }
  mock_resource "aws_vpc" {
    defaults = { arn = "arn:aws:ec2:us-east-1:123456789012:vpc/vpc-0abc" }
  }
  mock_resource "aws_vpc_endpoint" {
    defaults = { prefix_list_id = "pl-0123456789abcdef0" }
  }
  mock_resource "aws_iam_policy" {
    defaults = { arn = "arn:aws:iam::123456789012:policy/ratchet/mock" }
  }
}

variables {
  sandbox_id        = "demo"
  region            = "us-east-1"
  availability_zone = "us-east-1a"
}

# ---- DNS Firewall (default on) ----

run "dns_firewall_blocks_everything_but_the_allow_list" {
  command = apply

  assert {
    condition     = length(aws_route53_resolver_firewall_rule_group_association.sandbox) == 1
    error_message = "the firewall rule group is associated with the VPC by default"
  }

  assert {
    condition     = aws_route53_resolver_firewall_rule_group_association.sandbox[0].vpc_id == aws_vpc.sandbox.id
    error_message = "the association targets the sandbox VPC (bench and agent subnets share it)"
  }

  assert {
    condition     = aws_route53_resolver_firewall_rule.allow_listed[0].action == "ALLOW" && aws_route53_resolver_firewall_rule.block_everything_else[0].action == "BLOCK"
    error_message = "one ALLOW rule for the allow-list and one BLOCK rule for the rest"
  }

  assert {
    condition     = aws_route53_resolver_firewall_rule.allow_listed[0].priority < aws_route53_resolver_firewall_rule.block_everything_else[0].priority
    error_message = "the allow rule must be evaluated before the block-all rule"
  }

  assert {
    condition     = aws_route53_resolver_firewall_rule.block_everything_else[0].block_response == "NXDOMAIN"
    error_message = "blocked names answer NXDOMAIN"
  }

  assert {
    condition     = toset(aws_route53_resolver_firewall_domain_list.everything[0].domains) == toset(["*"])
    error_message = "the block rule matches every name"
  }

  assert {
    condition     = aws_route53_resolver_firewall_config.sandbox[0].firewall_fail_open == "DISABLED"
    error_message = "the firewall fails closed"
  }
}

run "dns_firewall_allows_the_bench_endpoints_and_internal_names" {
  command = apply

  assert {
    condition = alltrue([
      for d in [
        "ssm.us-east-1.amazonaws.com",
        "ssmmessages.us-east-1.amazonaws.com",
        "ec2messages.us-east-1.amazonaws.com",
        "logs.us-east-1.amazonaws.com",
        "s3.us-east-1.amazonaws.com",
        "*.s3.us-east-1.amazonaws.com",
      ] : contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, d)
    ])
    error_message = "the allow-list must cover the SSM, ssmmessages, ec2messages, logs and S3 names of the region"
  }

  assert {
    condition     = contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, "*.ec2.internal") && contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, "*.42.10.in-addr.arpa")
    error_message = "the VPC's own internal and reverse names stay resolvable"
  }

  assert {
    condition     = !anytrue([for d in aws_route53_resolver_firewall_domain_list.allowed[0].domains : d == "*" || d == "*.com" || d == "*.amazonaws.com"])
    error_message = "the allow-list must never contain a catch-all"
  }

  assert {
    condition     = alltrue([for d in aws_route53_resolver_firewall_domain_list.allowed[0].domains : endswith(d, ".us-east-1.amazonaws.com") || !strcontains(d, "amazonaws") || d == "s3.amazonaws.com" || d == "*.s3.amazonaws.com"])
    error_message = "AWS names are limited to the configured region (plus the legacy global S3 name in us-east-1)"
  }
}

run "dns_firewall_shared_vpc_keeps_agent_domains" {
  command = apply

  variables {
    egress_proxy_extra_domains = ["registry.npmjs.org"]
    dns_firewall_extra_domains = ["*.pythonhosted.org", "pypi.org"]
  }

  assert {
    condition = alltrue([
      for d in ["api.anthropic.com", "api.openai.com", "github.com", "*.github.com", "githubusercontent.com", "*.githubusercontent.com", "registry.npmjs.org", "pypi.org", "*.pythonhosted.org", "ec2.us-east-1.amazonaws.com", "sts.us-east-1.amazonaws.com"] :
      contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, d)
    ])
    error_message = "agent runners share the VPC, so the model APIs, GitHub, the AWS APIs they call and the extras stay resolvable"
  }

  assert {
    condition     = !contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, ".github.com")
    error_message = "squid's leading-dot syntax must be translated, not copied"
  }
}

run "dns_firewall_with_proxy_allows_package_bootstrap" {
  command = apply

  variables {
    enable_egress_proxy = true
  }

  assert {
    condition     = contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, "cdn.amazonlinux.com")
    error_message = "the proxy instance installs squid with dnf at boot"
  }
}

run "dns_firewall_allow_list_follows_the_region" {
  command = apply

  variables {
    region            = "eu-west-1"
    availability_zone = "eu-west-1a"
  }

  assert {
    condition     = contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, "logs.eu-west-1.amazonaws.com") && !contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, "logs.us-east-1.amazonaws.com")
    error_message = "service names use var.region"
  }

  assert {
    condition     = !contains(aws_route53_resolver_firewall_domain_list.allowed[0].domains, "s3.amazonaws.com")
    error_message = "the legacy global S3 name is only for us-east-1"
  }
}

run "dns_firewall_can_be_disabled" {
  command = apply

  variables {
    enable_dns_firewall = false
  }

  assert {
    condition     = length(aws_route53_resolver_firewall_rule_group_association.sandbox) == 0 && length(aws_route53_resolver_firewall_rule_group.sandbox) == 0 && length(aws_route53_resolver_firewall_config.sandbox) == 0
    error_message = "no firewall resources when disabled"
  }
}

run "dns_firewall_catch_all_extra_domain_rejected" {
  command = plan

  variables {
    dns_firewall_extra_domains = ["*"]
  }

  expect_failures = [var.dns_firewall_extra_domains]
}

run "dns_firewall_tld_wildcard_extra_domain_rejected" {
  command = plan

  variables {
    dns_firewall_extra_domains = ["*.com"]
  }

  expect_failures = [var.dns_firewall_extra_domains]
}

# ---- bench-inputs/ lifecycle ----

run "bench_inputs_expire_after_seven_days_by_default" {
  command = apply

  assert {
    condition = anytrue([
      for r in aws_s3_bucket_lifecycle_configuration.artifacts.rule :
      r.status == "Enabled" && r.filter[0].prefix == "bench-inputs/" && r.expiration[0].days == 7
      if r.id == "expire-bench-inputs"
    ])
    error_message = "bench-inputs/ expires after 7 days by default"
  }

  assert {
    condition = anytrue([
      for r in aws_s3_bucket_lifecycle_configuration.artifacts.rule :
      r.noncurrent_version_expiration[0].noncurrent_days == 7
      if r.id == "expire-bench-inputs"
    ])
    error_message = "noncurrent versions of bench inputs expire too, in case versioning is turned on"
  }

  assert {
    condition = anytrue([
      for r in aws_s3_bucket_lifecycle_configuration.artifacts.rule :
      r.expiration[0].days == 30
      if r.id == "expire-artifacts"
    ])
    error_message = "the general artifacts rule stays"
  }
}

run "bench_inputs_retention_is_configurable" {
  command = apply

  variables {
    bench_inputs_retention_days = 2
  }

  assert {
    condition = anytrue([
      for r in aws_s3_bucket_lifecycle_configuration.artifacts.rule :
      r.expiration[0].days == 2 && r.noncurrent_version_expiration[0].noncurrent_days == 2
      if r.id == "expire-bench-inputs"
    ])
    error_message = "bench_inputs_retention_days drives both expirations"
  }
}

run "bench_inputs_zero_days_rejected" {
  command = plan

  variables {
    bench_inputs_retention_days = 0
  }

  expect_failures = [var.bench_inputs_retention_days]
}

# ---- dependency snapshots must not open the bench subnet ----
#
# The deps job runs in the AGENT subnet (the only place with egress). Adding it
# changes nothing for the bench: same private route table, same five egress
# rules, and no package-registry name in the allow-list unless the operator
# adds one for the agent path.

run "deps_snapshots_leave_the_bench_subnet_offline" {
  command = apply

  assert {
    condition     = length(aws_route_table.private.route) == 0 && aws_route_table_association.bench.route_table_id == aws_route_table.private.id
    error_message = "the bench subnet keeps the route-less private table"
  }

  assert {
    condition     = length(aws_security_group.bench.egress) == 5 && !anytrue([for r in aws_security_group.bench.egress : contains(coalesce(r.cidr_blocks, []), "0.0.0.0/0")])
    error_message = "the bench security group keeps exactly its five rules and no internet egress"
  }

  assert {
    condition     = length(aws_security_group.bench.ingress) == 1 && one(aws_security_group.bench.ingress).self == true
    error_message = "the bench security group keeps self-only ingress"
  }

  assert {
    condition     = aws_subnet.bench.map_public_ip_on_launch == false
    error_message = "the bench subnet hands out no public IPs"
  }

  assert {
    condition     = !anytrue([for d in aws_route53_resolver_firewall_domain_list.allowed[0].domains : strcontains(d, "npmjs")])
    error_message = "no package registry is allowed by default: it is an explicit operator choice (egress_proxy_extra_domains / dns_firewall_extra_domains)"
  }

  assert {
    condition     = length(aws_security_group.agent_runner.ingress) == 0
    error_message = "the deps job reuses the agent runner group, which has no inbound"
  }
}
