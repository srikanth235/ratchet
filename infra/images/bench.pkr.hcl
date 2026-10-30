# Trusted bench image: what `AwsCloudCompute.launchBenchInstance` boots (LiveOptions.benchImageRef, or found by tag).
#
# GENERIC: the Docker engine, the SSM agent, the AWS CLI, zstd, git, iptables, the unprivileged `ratchet-bench` and
# `ratchet-holdout` users and the setuid launcher the evaluator harness uses inside the repo's container. The repo's
# toolchain (node, python, k6, ...) is NOT here: it is the repo's container image (`environment.image`), built once by the
# dependency/image job, saved to the artifacts bucket and loaded offline on bench instances (docs/real-env/bench.md).
# Deliberately NOT here either: the `claude`/`codex` CLIs, any model key or token, the ratchet-runner bin, compilers.
#
# `-var host_toolchain=true` adds node, pnpm, k6 and hyperfine for the legacy no-container path (a repo without
# `environment.image`). That image is tagged `ratchet:image=bench-host`, so tag lookup never picks it; pass its ami id explicitly.
#
# The IMDS block for ratchet-bench is added by the instance user data on every boot
# (packages/aws `benchUserData`), not baked in; 70-bench-user.sh and 90-verify-bench.sh only
# provide and check what that user data needs.

locals {
  bench_scripts = [
    "00-base.sh",
    "20-aws-cli.sh",
    "25-docker.sh",
    "35-candidate-launcher.sh",
    "70-bench-user.sh",
    "90-verify-bench.sh",
    "99-cleanup.sh",
  ]
  # Added by -var host_toolchain=true; `sort` puts them back in numeric order.
  bench_host_scripts  = ["10-node.sh", "30-bench-tools.sh"]
  bench_run_scripts   = sort(concat(local.bench_scripts, var.host_toolchain ? local.bench_host_scripts : []))
  bench_support_files = ["lib.sh", "versions.sh", "ratchet-candidate-ctr.c"]

  bench_name = "ratchet-bench${var.host_toolchain ? "-host" : ""}-${var.image_version}-${var.arch}"
  bench_tags = merge(local.common_tags, {
    "ratchet:image" = var.host_toolchain ? "bench-host" : "bench"
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
  ami_description = "Ratchet trusted bench ${var.image_version} (${var.arch}): docker, git, aws, ssm-agent, zstd, iptables, ratchet-bench and ratchet-holdout users${var.host_toolchain ? ", host toolchain (node, pnpm, k6, hyperfine)" : ""}"
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
    sources     = [for f in concat(local.bench_support_files, local.bench_run_scripts) : "${path.root}/scripts/${f}"]
    destination = "${local.remote_dir}/"
  }

  provisioner "shell" {
    execute_command  = "{{ .Vars }} sudo -n -E bash '{{ .Path }}'"
    environment_vars = ["RATCHET_DNF_ARGS=${var.dnf_args}", "RATCHET_HOST_TOOLCHAIN=${var.host_toolchain ? 1 : 0}"]
    inline           = [for s in local.bench_run_scripts : "bash ${local.remote_dir}/${s}"]
  }
}
