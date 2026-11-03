# `tofu test` with a mocked AWS provider - no account or credentials needed.
# Run: tofu init -backend=false && tofu test

mock_provider "aws" {
  mock_data "aws_ami" {
    defaults = { id = "ami-0bench0000000000" }
  }
  mock_resource "aws_instance" {
    defaults = {
      id                = "i-0123456789abcdef0"
      private_ip        = "10.42.1.10"
      availability_zone = "us-east-1a"
    }
  }
  mock_resource "aws_ebs_volume" {
    defaults = { id = "vol-0123456789abcdef0" }
  }
}

variables {
  ratchet = {
    sandbox_id                 = "demo"
    region                     = "us-east-1"
    vpc_id                     = "vpc-0abc"
    bench_subnet_id            = "subnet-0bench"
    bench_security_group_id    = "sg-0bench"
    artifacts_bucket           = "ratchet-demo-artifacts"
    eval_instance_profile_name = "ratchet-demo-bench-runner"
    environment_hash           = "abc1234567"
    tags                       = { team = "search" }
  }
  machines = {
    api = { role = "app", instance_type = "c7i.2xlarge", label = "API" }
    db  = { role = "database", instance_type = "r7g.large", data_volume_gb = 100 }
  }
}

run "machines_are_isolated_tagged_and_hardened" {
  command = apply

  assert {
    condition     = alltrue([for k, m in aws_instance.machine : m.subnet_id == "subnet-0bench"])
    error_message = "every machine goes into the private bench subnet"
  }

  assert {
    condition     = alltrue([for k, m in aws_instance.machine : m.associate_public_ip_address == false])
    error_message = "no machine gets a public address"
  }

  assert {
    condition     = alltrue([for k, m in aws_instance.machine : contains(m.vpc_security_group_ids, "sg-0bench") && length(m.vpc_security_group_ids) == 1])
    error_message = "every machine is in the bench group and no other"
  }

  assert {
    condition     = alltrue([for k, m in aws_instance.machine : m.iam_instance_profile == "ratchet-demo-bench-runner"])
    error_message = "every machine runs as the evaluation-only identity"
  }

  assert {
    condition     = alltrue([for k, m in aws_instance.machine : m.metadata_options[0].http_tokens == "required" && m.metadata_options[0].http_put_response_hop_limit == 1])
    error_message = "IMDSv2 is required"
  }

  assert {
    condition     = alltrue([for k, m in aws_instance.machine : m.root_block_device[0].encrypted == true && m.instance_initiated_shutdown_behavior == "terminate"])
    error_message = "volumes are encrypted and a shutdown terminates the machine"
  }

  assert {
    condition = alltrue([
      for k, m in aws_instance.machine :
      m.tags["ratchet:sandbox"] == "demo" && m.tags["ratchet:env"] == "abc1234567" && m.tags["ratchet:purpose"] == "environment" && m.tags["team"] == "search"
    ])
    error_message = "every machine carries the sandbox and environment tags plus the user's tags"
  }

  assert {
    condition     = aws_instance.machine["api"].tags["ratchet:env-role"] == "app" && aws_instance.machine["db"].tags["ratchet:env-role"] == "database"
    error_message = "machines are tagged with their role"
  }

  assert {
    condition     = length(aws_ebs_volume.data) == 1 && aws_ebs_volume.data["db"].encrypted == true && aws_ebs_volume.data["db"].tags["ratchet:env"] == "abc1234567"
    error_message = "only machines that ask for a data volume get one, encrypted and tagged"
  }

  assert {
    condition     = length(data.aws_ami.bench) == 1
    error_message = "without an image id the bench image is found by tag"
  }

  assert {
    condition     = output.environment.hash == "abc1234567" && output.tags["ratchet:purpose"] == "environment"
    error_message = "outputs echo the environment hash and tags"
  }
}

run "an_explicit_image_skips_the_lookup" {
  command = plan

  variables {
    ratchet = {
      sandbox_id                 = "demo"
      region                     = "us-east-1"
      vpc_id                     = "vpc-0abc"
      bench_subnet_id            = "subnet-0bench"
      bench_security_group_id    = "sg-0bench"
      artifacts_bucket           = "ratchet-demo-artifacts"
      eval_instance_profile_name = "ratchet-demo-bench-runner"
      environment_hash           = "abc1234567"
      machine_image_id           = "ami-0explicit0000000"
    }
  }

  assert {
    condition     = length(data.aws_ami.bench) == 0
    error_message = "an explicit machine_image_id is used as is"
  }
}

run "burstable_types_are_refused_for_measured_roles" {
  command = plan

  variables {
    machines = {
      api = { role = "app", instance_type = "t3.large" }
    }
  }

  expect_failures = [var.machines]
}

run "too_many_machines_are_refused" {
  command = plan

  variables {
    machines = {
      m1 = { role = "other", instance_type = "c7i.large" }
      m2 = { role = "other", instance_type = "c7i.large" }
      m3 = { role = "other", instance_type = "c7i.large" }
      m4 = { role = "other", instance_type = "c7i.large" }
      m5 = { role = "other", instance_type = "c7i.large" }
      m6 = { role = "other", instance_type = "c7i.large" }
      m7 = { role = "other", instance_type = "c7i.large" }
      m8 = { role = "other", instance_type = "c7i.large" }
      m9 = { role = "other", instance_type = "c7i.large" }
    }
  }

  expect_failures = [var.machines]
}

run "unknown_roles_are_refused" {
  command = plan

  variables {
    machines = {
      api = { role = "admin", instance_type = "c7i.large" }
    }
  }

  expect_failures = [var.machines]
}
