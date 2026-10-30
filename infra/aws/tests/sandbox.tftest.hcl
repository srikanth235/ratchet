# `tofu test` with a mocked AWS provider - no account or credentials needed.
# NOTE: written without access to an OpenTofu binary; not yet executed.

mock_provider "aws" {
  mock_data "aws_caller_identity" {
    defaults = { account_id = "123456789012" }
  }
  mock_data "aws_partition" {
    defaults = { partition = "aws" }
  }
  mock_data "aws_vpc" {
    defaults = {
      id         = "vpc-0abc"
      arn        = "arn:aws:ec2:us-east-1:123456789012:vpc/vpc-0abc"
      cidr_block = "10.0.0.0/16"
    }
  }
  mock_data "aws_subnets" {
    defaults = { ids = ["subnet-0bbb", "subnet-0aaa"] }
  }
  mock_resource "aws_s3_bucket" {
    defaults = { arn = "arn:aws:s3:::mock-bucket" }
  }
  mock_resource "aws_iam_policy" {
    defaults = { arn = "arn:aws:iam::123456789012:policy/ratchet/mock" }
  }
}

variables {
  sandbox_id = "demo"
  region     = "us-east-1"
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
      s.Sid == "DenyControlPlaneAndBilling" && s.Effect == "Deny" && contains(s.Action, "iam:*")
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
      s.Sid == "AllowRunInstancesTaggedAndAllowListed" &&
      s.Condition.StringEquals["ec2:InstanceType"] == ["c7i.large"] &&
      contains(keys(s.Condition.StringEquals), "aws:RequestTag/ratchet:opt")
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

run "agent_runner_sg_has_no_ingress" {
  command = apply

  assert {
    condition     = length(aws_security_group.agent_runner.ingress) == 0
    error_message = "agent runner must have no inbound rules"
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
