# Agent runner image: what `AwsCloudCompute.launchRunner` boots (LiveOptions.runnerImageRef).
#
# Node 22, pnpm, git, AWS CLI v2, the SSM agent, perf/strace/sysstat, hyperfine, k6, the pinned
# `claude` and `codex` CLIs and the `ratchet-runner` bin from this repository's packed tarballs
# (infra/images/pack-runner.sh). No model keys, no tokens, no repository: those arrive at launch
# through the instance profile, SSM SecureString parameters and user data.

locals {
  # Order matters. Shared steps first, runner-only steps after; the build fails in the verify step.
  runner_scripts = [
    "00-base.sh",
    "10-node.sh",
    "20-aws-cli.sh",
    "30-bench-tools.sh",
    "35-ratchet-shim.sh",
    "40-runner-tools.sh",
    "50-agent-clis.sh",
    "60-ratchet-runner.sh",
    "90-verify-runner.sh",
    "99-cleanup.sh",
  ]
  # Files uploaded next to the scripts: the helper library and the pins (the agent pins only here).
  runner_support_files = ["lib.sh", "versions.sh", "versions-agents.sh"]

  runner_name = "ratchet-runner-${var.image_version}-${var.arch}"
  runner_tags = merge(local.common_tags, {
    "ratchet:image" = "runner"
  })
}

source "amazon-ebs" "runner" {
  region        = var.region
  instance_type = local.builder_type
  source_ami    = var.source_ami

  dynamic "source_ami_filter" {
    for_each = var.source_ami == "" ? [1] : []
    content {
      filters = {
        name                = "al2023-ami-2023.*-${var.arch}"
        architecture        = var.arch
        root-device-type    = "ebs"
        virtualization-type = "hvm"
        state               = "available"
      }
      owners      = ["amazon"]
      most_recent = true
    }
  }

  ssh_username                = "ec2-user"
  ssh_timeout                 = "10m"
  vpc_id                      = var.vpc_id
  subnet_id                   = var.subnet_id
  associate_public_ip_address = true

  ami_name        = local.runner_name
  ami_description = "Ratchet agent runner ${var.image_version} (${var.arch}): Node 22, pnpm, git, claude, codex, aws, ssm-agent, perf, k6, hyperfine, ratchet-runner"
  ami_users       = var.ami_users
  encrypt_boot    = var.encrypt_boot
  kms_key_id      = var.kms_key_id
  imds_support    = "v2.0"

  # The builder follows the same metadata rules as everything else in the sandbox.
  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
  }

  launch_block_device_mappings {
    device_name           = "/dev/xvda"
    volume_size           = var.runner_volume_gb
    volume_type           = "gp3"
    delete_on_termination = true
  }

  tags          = merge(local.runner_tags, { Name = local.runner_name })
  snapshot_tags = local.runner_tags
  run_tags = {
    Name              = "packer-${local.runner_name}"
    "ratchet:builder" = "packer"
  }
}

build {
  name    = "runner"
  sources = ["source.amazon-ebs.runner"]

  provisioner "shell" {
    inline = ["mkdir -p ${local.remote_dir}/dist"]
  }

  provisioner "file" {
    sources     = [for f in concat(local.runner_support_files, local.runner_scripts) : "${path.root}/scripts/${f}"]
    destination = "${local.remote_dir}/"
  }

  # Output of infra/images/pack-runner.sh: ratchet-core-*.tgz, ratchet-agents-*.tgz, SHA256SUMS.
  provisioner "file" {
    source      = "${path.root}/dist/"
    destination = "${local.remote_dir}/dist/"
  }

  provisioner "shell" {
    execute_command  = "{{ .Vars }} sudo -n -E bash '{{ .Path }}'"
    environment_vars = ["RATCHET_DNF_ARGS=${var.dnf_args}", "RATCHET_RUNNER_DIST=${local.remote_dir}/dist"]
    inline           = [for s in local.runner_scripts : "bash ${local.remote_dir}/${s}"]
  }
}
