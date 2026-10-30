# Trusted bench image: what `AwsCloudCompute.launchBenchInstance` boots (LiveOptions.benchImageRef).
#
# Node 22, pnpm, git, AWS CLI v2, the SSM agent, k6, hyperfine, iptables and the unprivileged
# `ratchet-bench` user. Deliberately NOT here: the `claude`/`codex` CLIs, any model key or token,
# the ratchet-runner bin, profilers/compilers. Candidate code runs on this machine, so it stays
# minimal (docs/real-env/bench.md).
#
# The IMDS block for ratchet-bench is added by the instance user data on every boot
# (packages/aws `benchUserData`), not baked in; 70-bench-user.sh and 90-verify-bench.sh only
# provide and check what that user data needs.

locals {
  bench_scripts = [
    "00-base.sh",
    "10-node.sh",
    "20-aws-cli.sh",
    "30-bench-tools.sh",
    "35-ratchet-shim.sh",
    "70-bench-user.sh",
    "90-verify-bench.sh",
    "99-cleanup.sh",
  ]
  bench_support_files = ["lib.sh", "versions.sh"]

  bench_name = "ratchet-bench-${var.image_version}-${var.arch}"
  bench_tags = merge(local.common_tags, {
    "ratchet:image" = "bench"
  })
}

source "amazon-ebs" "bench" {
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

  ami_name        = local.bench_name
  ami_description = "Ratchet trusted bench ${var.image_version} (${var.arch}): Node 22, pnpm, git, aws, ssm-agent, k6, hyperfine, iptables, ratchet-bench user"
  ami_users       = var.ami_users
  encrypt_boot    = var.encrypt_boot
  kms_key_id      = var.kms_key_id
  imds_support    = "v2.0"

  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
  }

  launch_block_device_mappings {
    device_name           = "/dev/xvda"
    volume_size           = var.bench_volume_gb
    volume_type           = "gp3"
    delete_on_termination = true
  }

  tags          = merge(local.bench_tags, { Name = local.bench_name })
  snapshot_tags = local.bench_tags
  run_tags = {
    Name              = "packer-${local.bench_name}"
    "ratchet:builder" = "packer"
  }
}

build {
  name    = "bench"
  sources = ["source.amazon-ebs.bench"]

  provisioner "shell" {
    inline = ["mkdir -p ${local.remote_dir}"]
  }

  # Only the files the bench image needs: no agent installer, no agent pins, no runner tarballs.
  provisioner "file" {
    sources     = [for f in concat(local.bench_support_files, local.bench_scripts) : "${path.root}/scripts/${f}"]
    destination = "${local.remote_dir}/"
  }

  provisioner "shell" {
    execute_command  = "{{ .Vars }} sudo -n -E bash '{{ .Path }}'"
    environment_vars = ["RATCHET_DNF_ARGS=${var.dnf_args}"]
    inline           = [for s in local.bench_scripts : "bash ${local.remote_dir}/${s}"]
  }
}
