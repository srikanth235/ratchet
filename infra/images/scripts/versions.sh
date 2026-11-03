# shellcheck shell=bash
# shellcheck disable=SC2034
# Single source of truth for every pinned version and checksum baked into the
# images. Sourced by the provisioning scripts (never executed) and read by
# infra/tests. To bump a tool: change the version AND its checksums together
# (the commands are in infra/images/README.md, "Bumping a pin").
#
# Rules (enforced by infra/tests/src/images.test.ts):
#   - exact versions only, no ranges, tags or "latest";
#   - every downloaded artifact has a checksum here, for both architectures;
#   - archives are SHA-256, npm tarballs are the registry's SHA-512 SRI value.

# Node.js 22 LTS, official tarballs (https://nodejs.org/dist/v$NODE_VERSION/SHASUMS256.txt).
NODE_VERSION=22.23.4
NODE_SHA256_x64=1805bcd35e6815559bd2eb97db438d3fdb0eb3a5e6a977b7c1badca76337e9ce
NODE_SHA256_arm64=6287aa46b6c375d816e63ae66d024fbbfd70404825aabdfdf898a970871d2f4b

# pnpm: the same version as the root package.json "packageManager" field.
PNPM_VERSION=10.28.0
PNPM_INTEGRITY=sha512-Bd9x0UIfITmeBT/eVnzqNNRG+gLHZXFEG/wceVbpjjYwiJgtlARl/TRIDU2QoGaLwSNi+KqIAApk6D0LDke+SA==

# AWS CLI v2 (awscli.amazonaws.com publishes only GPG signatures; these digests were computed
# from the downloaded zips).
AWSCLI_VERSION=2.39.1
AWSCLI_SHA256_x86_64=5b6c10e0a8ccdcf30c84bd643fea511f103d257a4787a7cef878691d63169804
AWSCLI_SHA256_aarch64=5adf0b5052843e542b55a846d49ba2a2b3f3fb4960f35c11df9ac7c395cffaaf

# k6 (the digests match the release's k6-v$K6_VERSION-checksums.txt).
K6_VERSION=1.8.2
K6_SHA256_amd64=37620781d35406ebf402397a6c5cf4adb62d175cb052ff45325ae0b860527063
K6_SHA256_arm64=8b4e55024ffc8ed7ed4caabca054dfb5bafe16e7ee4235de57d01bf58d69e0d7

# hyperfine
HYPERFINE_VERSION=1.21.0
HYPERFINE_SHA256_x86_64=60b70eb01e1a4dce1bf56a38393e1e2c2e79ed95a4ba711feb5c993f382604ab
HYPERFINE_SHA256_aarch64=7c3e7183e4178e5c1bb68ef714fbf2ac1cef04d4d7fc751d645d290ac0041d7a
