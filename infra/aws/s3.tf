# ---- Artifacts bucket: agent event streams and build outputs ----
# Agents reach it only under artifacts/<opt>/<agent>/ (enforced by the boundary).

resource "aws_s3_bucket" "artifacts" {
  bucket        = "${local.name_prefix}-artifacts-${local.account_id}"
  force_destroy = var.force_destroy
}

resource "aws_s3_bucket_public_access_block" "artifacts" {
  bucket                  = aws_s3_bucket.artifacts.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id

  rule {
    id     = "expire-artifacts"
    status = "Enabled"
    filter {}

    expiration {
      days = var.artifacts_retention_days
    }
    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

# ---- Sealed bucket: hidden traffic traces, bench runner only ----

resource "aws_s3_bucket" "sealed" {
  bucket        = "${local.name_prefix}-sealed-${local.account_id}"
  force_destroy = var.force_destroy
}

resource "aws_s3_bucket_public_access_block" "sealed" {
  bucket                  = aws_s3_bucket.sealed.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_ownership_controls" "sealed" {
  bucket = aws_s3_bucket.sealed.id
  rule {
    object_ownership = "BucketOwnerEnforced"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "sealed" {
  bucket = aws_s3_bucket.sealed.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_versioning" "sealed" {
  bucket = aws_s3_bucket.sealed.id
  versioning_configuration {
    status = "Enabled"
  }
}

# Explicit deny for every agent identity (matched by role path and, as a second
# net, by the ratchet:agent principal tag), plus TLS-only. An explicit deny in a
# resource policy wins over any identity policy the agent role might carry.
resource "aws_s3_bucket_policy" "sealed_deny_agents" {
  bucket = aws_s3_bucket.sealed.id
  policy = templatefile("${path.module}/policies/sealed_bucket_policy.json.tftpl", {
    sealed_bucket_arn      = aws_s3_bucket.sealed.arn
    agent_role_arn_pattern = "arn:${local.partition}:iam::${local.account_id}:role${local.agent_role_path}*"
  })

  # The public access block and the policy are both applied to the bucket;
  # serialise them to avoid conflicting concurrent PutBucket* calls.
  depends_on = [aws_s3_bucket_public_access_block.sealed]
}
