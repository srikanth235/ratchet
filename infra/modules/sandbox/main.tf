locals {
  tags = merge(var.ratchet.tags, {
    "ratchet:sandbox" = var.ratchet.sandbox_id
    "ratchet:purpose" = "environment"
    "ratchet:env"     = var.ratchet.environment_hash
    "ratchet:managed" = "opentofu"
  })

  prefix = "ratchet-${var.ratchet.sandbox_id}"

  machine_image_id = var.ratchet.machine_image_id != "" ? var.ratchet.machine_image_id : data.aws_ami.bench[0].id
  image_versions   = var.ratchet.image_version == "" ? ["*"] : [var.ratchet.image_version, "${var.ratchet.image_version}-*", "${var.ratchet.image_version}+*"]

  data_volumes = { for k, m in var.machines : k => m if m.data_volume_gb > 0 }
}

# The newest bench image Ratchet's Packer build tagged (infra/images). Only looked up when no image id was given.
data "aws_ami" "bench" {
  count = var.ratchet.machine_image_id == "" ? 1 : 0

  owners      = ["self"]
  most_recent = true

  filter {
    name   = "tag:ratchet:image"
    values = ["bench"]
  }

  filter {
    name   = "tag:ratchet:image_version"
    values = local.image_versions
  }
}

resource "aws_instance" "machine" {
  for_each = var.machines

  ami           = local.machine_image_id
  instance_type = each.value.instance_type

  # Placement is not the draft's to choose: the private bench subnet and the bench group only.
  subnet_id                   = var.ratchet.bench_subnet_id
  vpc_security_group_ids      = [var.ratchet.bench_security_group_id]
  associate_public_ip_address = false

  # Evaluation-only identity: reads staged inputs, writes bench results, no access to the sealed store.
  iam_instance_profile = var.ratchet.eval_instance_profile_name

  user_data                   = each.value.user_data == "" ? null : each.value.user_data
  user_data_replace_on_change = true

  instance_initiated_shutdown_behavior = "terminate"
  disable_api_termination              = false
  monitoring                           = false

  metadata_options {
    http_endpoint               = "enabled"
    http_tokens                 = "required"
    http_put_response_hop_limit = 1
    instance_metadata_tags      = "disabled"
  }

  root_block_device {
    volume_type           = "gp3"
    volume_size           = each.value.root_volume_gb
    encrypted             = true
    delete_on_termination = true
  }

  tags = merge(local.tags, {
    Name                  = "${local.prefix}-${each.key}"
    "ratchet:env-machine" = each.key
    "ratchet:env-role"    = each.value.role
  })

  volume_tags = merge(local.tags, {
    Name                  = "${local.prefix}-${each.key}"
    "ratchet:env-machine" = each.key
  })
}

resource "aws_ebs_volume" "data" {
  for_each = local.data_volumes

  availability_zone = aws_instance.machine[each.key].availability_zone
  size              = each.value.data_volume_gb
  type              = "gp3"
  encrypted         = true

  tags = merge(local.tags, {
    Name                  = "${local.prefix}-${each.key}-data"
    "ratchet:env-machine" = each.key
  })
}

resource "aws_volume_attachment" "data" {
  for_each = local.data_volumes

  device_name                    = "/dev/sdf"
  volume_id                      = aws_ebs_volume.data[each.key].id
  instance_id                    = aws_instance.machine[each.key].id
  stop_instance_before_detaching = true
}
