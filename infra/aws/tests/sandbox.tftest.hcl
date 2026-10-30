# `tofu test` with a mocked AWS provider - no account or credentials needed.
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
  sandbox_id = "demo"
  region     = "us-east-1"
  # mock_data list defaults are not applied, so pin the AZ instead of reading it.
  availability_zone = "us-east-1a"
}

run "boundary_denies_sealed_and_control_plane" {
  command = apply

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_policy.agent_boundary.policy).Statement :
      s.Sid == "DenySealedBucket" && s.Effect == "Deny"
    ])
    error_message = "boundary must explicitly deny the sealed bucket"
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_policy.agent_boundary.policy).Statement :
      s.Effect == "Deny" && contains(flatten([s.Action]), "iam:*")
      if try(s.Sid, "") == "DenyControlPlaneAndBilling"
    ])
    error_message = "boundary must deny iam:*"
  }

  assert {
    condition     = startswith(aws_iam_policy.agent_boundary.path, "/ratchet/")
    error_message = "boundary lives under /ratchet/"
  }
}

run "boundary_requires_tags_and_allow_listed_types" {
  command = apply

  variables {
    allowed_instance_types = ["c7i.large"]
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_policy.agent_boundary.policy).Statement :
      flatten([s.Condition.StringEquals["ec2:InstanceType"]]) == ["c7i.large"] &&
      contains(keys(s.Condition.StringEquals), "aws:RequestTag/ratchet:opt")
      if try(s.Sid, "") == "AllowRunInstancesTaggedAndAllowListed"
    ])
    error_message = "RunInstances must require the opt tag and only allow-listed types"
  }
}

run "sealed_policy_denies_agent_path" {
  command = apply

  assert {
    condition = anytrue([
      for s in jsondecode(aws_s3_bucket_policy.sealed_deny_agents.policy).Statement :
      s.Effect == "Deny" && try(s.Condition.ArnLike["aws:PrincipalArn"], "") == "arn:aws:iam::123456789012:role/ratchet/agent/*"
    ])
    error_message = "sealed bucket must deny roles under /ratchet/agent/"
  }
}

run "bench_role_outside_agent_path" {
  command = apply

  assert {
    condition     = aws_iam_role.bench_runner.path == "/ratchet/bench/"
    error_message = "bench runner must not live under the agent path"
  }
}

run "bench_roles_split_sealed_access" {
  command = apply

  assert {
    condition     = aws_iam_role.confirm_runner.path == "/ratchet/bench/" && aws_iam_role.confirm_runner.name != aws_iam_role.bench_runner.name
    error_message = "confirm runner is a separate role outside the agent path"
  }

  assert {
    condition     = !strcontains(aws_iam_role_policy.bench_runner.policy, "sealed")
    error_message = "the bench runner policy must not mention the sealed bucket"
  }

  assert {
    condition     = strcontains(aws_iam_role_policy.confirm_runner.policy, "ratchet-demo-sealed") || strcontains(aws_iam_role_policy.confirm_runner.policy, "holdout/")
    error_message = "the confirm runner policy must grant the holdout prefix"
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_role_policy.confirm_runner.policy).Statement :
      s.Sid == "ReadHoldoutTraces" && endswith(s.Resource, "/holdout/*")
    ])
    error_message = "confirm runner reads only the holdout prefix of the sealed bucket"
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_s3_bucket_policy.sealed_deny_agents.policy).Statement :
      s.Sid == "DenyBenchRunner" && s.Effect == "Deny"
    ])
    error_message = "sealed bucket must explicitly deny the bench runner"
  }
}

run "deps_runner_is_a_third_identity_that_only_writes_bench_deps" {
  command = apply

  assert {
    condition     = aws_iam_role.deps_runner.path == "/ratchet/bench/" && aws_iam_role.deps_runner.name != aws_iam_role.bench_runner.name && aws_iam_role.deps_runner.name != aws_iam_role.confirm_runner.name
    error_message = "the deps runner is a separate role outside the agent path"
  }

  assert {
    condition     = aws_iam_instance_profile.deps_runner.role == aws_iam_role.deps_runner.name && aws_iam_instance_profile.deps_runner.name != aws_iam_instance_profile.bench_runner.name
    error_message = "the deps runner has its own instance profile"
  }

  # Every S3 write the deps runner holds is on bench-deps/ and nothing else.
  assert {
    condition = alltrue([
      for s in jsondecode(aws_iam_role_policy.deps_runner.policy).Statement :
      endswith(s.Resource, "/bench-deps/*")
      if try(contains(flatten([s.Action]), "s3:PutObject"), false)
    ])
    error_message = "the deps runner may write only under bench-deps/"
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_role_policy.deps_runner.policy).Statement :
      s.Sid == "WriteBenchDeps" && s.Effect == "Allow" && endswith(s.Resource, "/bench-deps/*")
    ])
    error_message = "the deps runner can write bench-deps/"
  }

  # Reads: only the staged base tree, never the candidate archive.
  assert {
    condition = alltrue([
      for s in jsondecode(aws_iam_role_policy.deps_runner.policy).Statement :
      endswith(s.Resource, "/bench-inputs/*/deps-base.tgz")
      if try(contains(flatten([s.Action]), "s3:GetObject"), false)
    ])
    error_message = "the deps runner may read only the staged base tree (deps-base.tgz), not the candidate's code.tgz"
  }

  assert {
    condition     = !strcontains(aws_iam_role_policy.deps_runner.policy, "sealed") && !strcontains(aws_iam_role_policy.deps_runner.policy, "bench-results") && !strcontains(aws_iam_role_policy.deps_runner.policy, "artifacts/")
    error_message = "the deps runner policy mentions neither the sealed bucket, bench-results/ nor the agent prefixes"
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_iam_role_policy.deps_runner.policy).Statement :
      !anytrue([for a in flatten([s.Action]) : startswith(a, "iam:") || startswith(a, "sts:") || startswith(a, "ssm:Get") || startswith(a, "ec2:") || a == "*" || a == "s3:*"])
    ])
    error_message = "the deps runner has no IAM, STS, EC2, parameter or wildcard actions"
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_s3_bucket_policy.sealed_deny_agents.policy).Statement :
      s.Sid == "DenyDepsRunner" && s.Effect == "Deny"
    ])
    error_message = "the sealed bucket must explicitly deny the deps runner"
  }

  assert {
    condition = alltrue([
      for k in ["role_arn", "role_name", "instance_profile_arn", "instance_profile_name"] :
      contains(keys(output.deps_identity_ref), k)
    ])
    error_message = "deps_identity_ref has the same shape as eval_identity_ref"
  }
}

run "bench_and_confirm_read_dependency_snapshots_but_cannot_write_them" {
  command = apply

  assert {
    condition = alltrue([
      for p in [aws_iam_role_policy.bench_runner.policy, aws_iam_role_policy.confirm_runner.policy] :
      anytrue([for s in jsondecode(p).Statement : s.Sid == "ReadBenchDeps" && s.Action == "s3:GetObject" && endswith(s.Resource, "/bench-deps/*")])
    ])
    error_message = "bench and confirm runners read bench-deps/"
  }

  assert {
    condition = alltrue([
      for p in [aws_iam_role_policy.bench_runner.policy, aws_iam_role_policy.confirm_runner.policy] :
      alltrue([
        for s in jsondecode(p).Statement :
        !strcontains(s.Resource, "bench-deps") if try(contains(flatten([s.Action]), "s3:PutObject"), false)
      ])
    ])
    error_message = "candidate-facing identities must not be able to write dependency snapshots"
  }
}

run "custom_confirm_prefix_is_used" {
  command = apply

  variables {
    sealed_confirm_prefix = "traces/v1/"
  }

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_role_policy.confirm_runner.policy).Statement :
      s.Sid == "ReadHoldoutTraces" && endswith(s.Resource, "/traces/v1/*")
    ])
    error_message = "confirm runner follows sealed_confirm_prefix"
  }
}

run "wildcard_confirm_prefix_rejected" {
  command = plan

  variables {
    sealed_confirm_prefix = "*/"
  }

  expect_failures = [var.sealed_confirm_prefix]
}

run "traversal_confirm_prefix_rejected" {
  command = plan

  variables {
    sealed_confirm_prefix = "../"
  }

  expect_failures = [var.sealed_confirm_prefix]
}

run "agent_runner_sg_has_no_ingress" {
  command = apply

  assert {
    condition     = length(aws_security_group.agent_runner.ingress) == 0
    error_message = "agent runner must have no inbound rules"
  }
}

run "bench_has_no_internet_egress" {
  command = apply

  assert {
    condition = alltrue([
      for r in aws_security_group.bench.egress :
      length(coalesce(r.cidr_blocks, [])) == 0 || (r.from_port == 53 && r.to_port == 53)
    ])
    error_message = "bench egress may use CIDRs only for DNS to the VPC; everything else is self / endpoint group / prefix list"
  }

  assert {
    condition     = !anytrue([for r in aws_security_group.bench.egress : contains(coalesce(r.cidr_blocks, []), "0.0.0.0/0")])
    error_message = "bench must have no 0.0.0.0/0 egress"
  }

  assert {
    condition     = length(aws_route_table.private.route) == 0
    error_message = "the private route table (bench) must have no routes beyond local"
  }
}

run "bench_subnet_is_private_and_endpoints_exist" {
  command = apply

  assert {
    condition     = aws_subnet.bench.map_public_ip_on_launch == false
    error_message = "bench subnet must not hand out public IPs"
  }

  assert {
    condition     = aws_vpc_endpoint.s3.vpc_endpoint_type == "Gateway"
    error_message = "S3 must be a gateway endpoint"
  }

  assert {
    condition     = toset([for k, e in aws_vpc_endpoint.interface : k]) == toset(["ssm", "ssmmessages", "ec2messages", "logs"])
    error_message = "interface endpoints for ssm, ssmmessages, ec2messages and logs"
  }

  assert {
    condition     = alltrue([for k, e in aws_vpc_endpoint.interface : e.private_dns_enabled])
    error_message = "interface endpoints need private DNS"
  }

  assert {
    condition = alltrue([
      for s in jsondecode(aws_vpc_endpoint.s3.policy).Statement :
      alltrue([for r in flatten([s.Resource]) : strcontains(r, "mock-bucket")])
    ])
    error_message = "the S3 endpoint policy admits only the sandbox buckets"
  }
}

run "proxy_disabled_by_default" {
  command = apply

  assert {
    condition     = length(aws_instance.egress_proxy) == 0 && length(aws_security_group.egress_proxy) == 0
    error_message = "no proxy resources unless enable_egress_proxy"
  }

  assert {
    condition     = aws_subnet.agent.map_public_ip_on_launch == true
    error_message = "without the proxy the agent subnet is public"
  }

  assert {
    condition     = output.network_ref.egress_proxy_url == null
    error_message = "no proxy url when disabled"
  }
}

run "proxy_enabled_removes_runner_internet" {
  command = apply

  variables {
    enable_egress_proxy        = true
    egress_proxy_extra_domains = ["registry.yarnpkg.com"]
  }

  assert {
    condition     = length(aws_instance.egress_proxy) == 1 && aws_instance.egress_proxy[0].instance_type == "t4g.nano"
    error_message = "one t4g.nano proxy"
  }

  assert {
    condition     = aws_subnet.agent.map_public_ip_on_launch == false
    error_message = "with the proxy the agent subnet has no public IPs"
  }

  assert {
    condition     = !anytrue([for r in aws_security_group.agent_runner.egress : contains(coalesce(r.cidr_blocks, []), "0.0.0.0/0")])
    error_message = "runner egress must not be open to the internet when the proxy is on"
  }

  assert {
    condition     = strcontains(aws_instance.egress_proxy[0].user_data, "api.anthropic.com") && strcontains(aws_instance.egress_proxy[0].user_data, "registry.npmjs.org") && strcontains(aws_instance.egress_proxy[0].user_data, "registry.yarnpkg.com") && strcontains(aws_instance.egress_proxy[0].user_data, "http_access deny all")
    error_message = "squid config carries the allow-list, the extra domains and a final deny"
  }

  assert {
    condition     = startswith(output.network_ref.egress_proxy_url, "http://10.42.3.10:")
    error_message = "network_ref exposes the proxy url"
  }

  assert {
    condition     = aws_instance.egress_proxy[0].metadata_options[0].http_tokens == "required"
    error_message = "proxy requires IMDSv2"
  }
}

run "proxy_domain_wildcards_rejected" {
  command = plan

  variables {
    egress_proxy_extra_domains = ["*.example.com"]
  }

  expect_failures = [var.egress_proxy_extra_domains]
}

run "boundary_cannot_launch_into_bench" {
  command = apply

  assert {
    condition = anytrue([
      for s in jsondecode(aws_iam_policy.agent_boundary.policy).Statement :
      s.Sid == "DenyRunInstancesInInternalSecurityGroups" && s.Effect == "Deny"
    ])
    error_message = "boundary denies launching into the bench/endpoint security groups"
  }
}

run "reserved_tags_rejected" {
  command = plan

  variables {
    tags = { "ratchet:opt" = "x" }
  }

  expect_failures = [var.tags]
}

run "wildcard_instance_types_rejected" {
  command = plan

  variables {
    allowed_instance_types = ["c7i.*"]
  }

  expect_failures = [var.allowed_instance_types]
}
