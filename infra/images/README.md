# infra/images

Packer definitions for the two machine images that live mode launches. Nothing else builds them.

| Image | Launched by | Passed to live mode as | Contains |
|---|---|---|---|
| **runner** | `AwsCloudCompute.launchRunner` | `runnerImageRef` | Node 22, pnpm, git, AWS CLI v2, SSM agent, `claude` and `codex` (pinned), perf/strace/sysstat, hyperfine, k6, gcc/make/python3, the `ratchet-runner` bin from this repository |
| **bench** | `AwsCloudCompute.launchBenchInstance` | `benchImageRef` (override only; normally found by tag) | **Generic**: the Docker engine, git, AWS CLI v2, SSM agent, iptables, zstd, the unprivileged `ratchet-bench` and `ratchet-holdout` users and the setuid `ratchet-candidate-ctr` launcher (the dependency-snapshot / container-image job boots this image too, as `purpose: 'deps'`; its `ratchet-deps` user is created by that instance's user data). The toolchain is the repo's container (`environment.image`), built once and loaded offline. **No language toolchain, no compiler, no agent CLIs, no model keys, no runner bin** |

Both are Amazon Linux 2023 (`x86_64` by default, `arm64` with `-var arch=arm64`; build the architecture of the instance types you launch).

```
infra/images/
  common.pkr.hcl      plugin pin, variables, shared locals (tags, builder type)
  runner.pkr.hcl      source + build for the runner image (script list = `runner_scripts`)
  bench.pkr.hcl       source + build for the bench image (script list = `bench_scripts`)
  build.sh            packs the runner tarballs (runner only), runs packer, prints the AMI ids
  pack-runner.sh      builds + `pnpm pack`s @ratchet/core and @ratchet/agents into dist/ with SHA256SUMS
  scripts/
    versions.sh         pinned versions + checksums (node, pnpm, aws cli, k6, hyperfine)
    versions-agents.sh  pinned versions + integrity (claude, codex); never uploaded to the bench builder
    lib.sh              fetch/verify helpers shared by the steps
    00-base.sh          dnf: git, tar, util-linux, procps, findutils, grep, SSM agent        (both)
    10-node.sh          Node tarball (sha256) + pnpm (npm tarball, sha512)                    (runner; bench only with host_toolchain)
    20-aws-cli.sh       AWS CLI v2 zip (sha256)                                               (both)
    25-docker.sh        Docker engine (dnf), daemon.json, enabled; nobody in group docker     (bench)
    30-bench-tools.sh   k6 + hyperfine (sha256)                                               (runner; bench only with host_toolchain)
    35-candidate-launcher.sh  compiles ratchet-candidate-ctr.c statically, installs it setuid root
                        in a 0700 directory, removes the compiler                             (bench)
    ratchet-candidate-ctr.c  the launcher the evaluator harness uses inside the repo container (bench)
    40-runner-tools.sh  perf, strace, sysstat, gcc, make, python3                             (runner)
    50-agent-clis.sh    claude + codex, pinned + sha512 checked                               (runner)
    60-ratchet-runner.sh  installs the packed tarballs, adds the `ratchet-runner` launcher    (runner)
    70-bench-user.sh    iptables-nft + the `ratchet-bench` user                               (bench)
    90-verify-*.sh      build-time smoke test; a failure fails the Packer build               (each)
    99-cleanup.sh       caches, logs, cloud-init state, build ssh keys                        (both)
```

## Build images

Prerequisites: Packer >= 1.10 (validated with 1.16.2), AWS credentials for the **sandbox account** (AMIs are
regional and, encrypted by default, only launchable from the account that built them), and for the runner
image `pnpm` + Node 22 on the machine running the build.

```sh
export AWS_PROFILE=ratchet-sandbox AWS_REGION=us-east-1
infra/images/build.sh both            # or: runner | bench
# ...
# runnerImageRef: ami-0123456789abcdef0
# benchImageRef: ami-0fedcba9876543210
```

Equivalent by hand:

```sh
infra/images/pack-runner.sh                                   # runner only: writes infra/images/dist/
packer init infra/images
packer build -only='runner.*' -var image_version=20261030-1a2b3c4 -var source_commit=1a2b3c4 infra/images
packer build -only='bench.*'  -var image_version=20261030-1a2b3c4 infra/images
```

Useful variables (all in `common.pkr.hcl`): `region`, `arch`, `instance_type`, `source_ami`, `dnf_args`,
`vpc_id`/`subnet_id`, `encrypt_boot`/`kms_key_id`, `ami_users`, `runner_volume_gb`, `bench_volume_gb`, `extra_tags`,
and `host_toolchain` (bench only, default `false`: also install node, pnpm, k6 and hyperfine for repos without `environment.image`;
the result is named `ratchet-bench-host-...` and tagged `ratchet:image=bench-host`, so the tag lookup never selects it; pass its id as
`benchImageRef`).

- The **builder needs outbound internet** (dnf, npm, nodejs.org, GitHub releases, awscli.amazonaws.com). Build in
  the default VPC or a public subnet; the sandbox's bench subnet has no route out and the agent subnet may be behind
  the egress proxy, so neither works. Nothing is downloaded at run time: a runner behind the proxy needs only the
  model APIs and GitHub, not the package hosts.
- The Packer caller needs the EC2 permissions of the amazon-ebs builder (see the plugin docs: run/stop/terminate
  instances, create/register/deregister images and snapshots, keypairs and security groups, describe calls, and `ec2:CreateTags`).
- Result: one AMI per image in the region, named `ratchet-<runner|bench>-<version>-<arch>`.

### Tags

Every AMI and snapshot carries:

| Tag | Value |
|---|---|
| `ratchet:image` | `runner` or `bench` (`bench-host` for the host-toolchain variant) |
| `ratchet:image_version` | `-var image_version`. `build.sh`: `<app version>-<UTC date>-<commit>` (`-dirty` suffix for an unclean tree); the app finds images whose version equals its own or starts with `<its version>-` |
| `ratchet:source_commit` | commit the runner tarballs were packed from |
| `ratchet:arch` | `x86_64` or `arm64` |
| `ratchet:managed` | `packer` |

Find the newest image of a kind:

```sh
aws ec2 describe-images --owners self --filters Name=tag:ratchet:image,Values=runner \
  --query 'sort_by(Images,&CreationDate)[-1].[ImageId,Name]' --output text
```

### Finding the images (no AMI ids to type)

Live mode finds the images by tag: `findRatchetImages(appVersion, { region, arch })` (`packages/aws/src/images.ts`) calls EC2
`DescribeImages` (own account, `state=available`, the architecture) filtered on `tag:ratchet:image` and picks, per kind, the newest
whose `ratchet:image_version` matches the app version. `createLiveRuntimeFor` / `resolveLiveImages` (`packages/runtime/src/live.ts`)
wire it in, in the region of the optimization (`environment.region`). Build the images in the sandbox account and region **with the
app's version** (`build.sh` does) or the lookup fails with a message naming the image to build and the versions that exist.

The explicit override is for development only (Settings > Advanced):
`createLiveRuntime({ ..., runnerImageRef: 'ami-...', benchImageRef: 'ami-...' })`. The runner AMI id also reaches the engine as
`settings.runnerImageRef`. Both are plain `ami-...` ids in the sandbox's region. See `docs/real-env/aws.md` and `docs/real-env/agents.md`.

## How the images and the instance user data fit together

The launcher (`packages/aws/src/compute/aws.ts`) boots these images with user data. The image provides what the user
data needs; it does not repeat what the user data does.

| | Image (this directory) | User data (`runnerUserData` / `benchUserData`) |
|---|---|---|
| Bench users `ratchet-bench`, `ratchet-holdout` | create them with the same flags (`useradd --system --no-create-home --shell /usr/sbin/nologin`); user data's `id -u \|\| useradd` is then a no-op | creates them only if missing |
| Docker engine | installs and enables it (`25-docker.sh`), writes `daemon.json` (no tcp socket, no seccomp or `no-new-privileges` override), puts nobody in group `docker`, ships no layers | bench: `systemctl start docker` when present. Image/deps job: also rejects metadata for forwarded container traffic (`DOCKER-USER`, `FORWARD`) and gives the daemon the egress-proxy settings |
| Container launcher | `35-candidate-launcher.sh` installs `/usr/local/libexec/ratchet/ratchet-candidate-ctr` (4755 root, 0700 directory, static, no compiler left) | nothing: the bench step script bind-mounts it read-only into the harness container together with a root-owned `launcher.conf` |
| IMDS block for that user | installs `iptables-nft` (iptables and ip6tables; the user data aborts under `set -e` without them) and, at build time only, proves the kernel accepts the same owner rule, then deletes it. **No rule is baked in, no `iptables-services`** | inserts `iptables -I OUTPUT 1 -m owner --uid-owner ratchet-bench -d 169.254.169.254 -j REJECT` and the IPv6 twin on every boot; the bench runner refuses a job without it |
| Metadata options | AMI default `imds_support = v2.0` | `RunInstances`: IMDSv2 required, hop limit 1 |
| `/etc/ratchet/runner.env` (secret refs, ids, proxy) | nothing; the build fails if it exists | writes it |
| Lifetime guard (`shutdown -h +N`) | nothing | sets it |
| Runner loop / bench jobs | nothing starts at boot | touches `/run/ratchet/user-data.done`; the app starts `ratchet-runner` and bench jobs over SSM |

One detail worth knowing:

- **The launcher.** `SsmRunnerControl.start` runs `env RATCHET_...=... ratchet-runner` from a plain SSM shell, which
  has not seen the user data's `/etc/ratchet/runner.env` (`RATCHET_SECRET_REFS`, proxy variables). The installed
  `/usr/local/bin/ratchet-runner` loads that file first; variables the caller already set win. The runner bin itself
  is untouched.

## Reproducibility

- Tools: exact versions and SHA-256 / SHA-512 values in `scripts/versions*.sh`; a mismatch fails the build. For the
  npm CLIs only the top-level tarball is pinned; their platform binaries come from `optionalDependencies` at the same
  exact version and are verified by npm against the registry metadata.
- AWS CLI and k6 / hyperfine / Node checksums are the ones published with the releases where available (Node:
  `SHASUMS256.txt`, k6: `*-checksums.txt`); the AWS CLI and hyperfine publish none, so those were computed from the
  downloads.
- OS packages: Amazon Linux 2023 repositories are versioned by release. For identical rebuilds pin both the base image
  and the repository snapshot: `-var source_ami=ami-... -var dnf_args=--releasever=2023.x.YYYYMMDD`.
- Not pinned: the SSM agent and the dnf packages (they follow the base image / `dnf_args`), and the GPG signatures of
  the downloads are not checked (only digests).

## Bumping a pin

```sh
# Node: take both lines from https://nodejs.org/dist/v<ver>/SHASUMS256.txt (linux-x64.tar.xz, linux-arm64.tar.xz)
# npm package (pnpm, claude, codex): the registry integrity of the exact version
npm view @anthropic-ai/claude-code@<ver> dist.integrity
# archives (aws cli, k6, hyperfine): download, then
sha256sum awscli-exe-linux-x86_64-<ver>.zip
```

Edit `scripts/versions.sh` / `versions-agents.sh`, run `pnpm --filter @ratchet/infra-tests test`, rebuild. Bump the CLI
pins together with a run of the adapter tests (`docs/real-env/agents.md`): the adapters parse their output formats.
`PNPM_VERSION` must equal the root `package.json` `packageManager`.

## Validating without AWS

```sh
pnpm --filter @ratchet/infra-tests test                       # invariants of scripts and templates (images.test.ts)
shellcheck -x -P SCRIPTDIR -s bash infra/images/scripts/*.sh infra/images/*.sh
packer init infra/images && packer validate -var image_version=test infra/images   # needs no AWS calls
packer validate -syntax-only infra/images                     # no plugin download
```

The install steps that need no root-only tooling (Node, pnpm, AWS CLI, k6, hyperfine, claude, codex, the runner
tarballs) can be exercised in a scratch directory:
`RATCHET_PREFIX=/tmp/p RATCHET_ETC=/tmp/e RATCHET_RUNNER_DIST=infra/images/dist bash infra/images/scripts/10-node.sh`.
The `dnf`, `useradd`, `systemctl` and `iptables` steps (and both verify scripts) only run on a real builder.

## Smoke test after a build

1. Launch the bench AMI through `AwsCloudCompute.launchBenchInstance` (docs/real-env/aws.md section 3) and run
   `docs/real-env/bench.md` section 5 (`iptables -C OUTPUT ...`, `sudo -u ratchet-bench node -v` equivalents, an
   isolation refusal with the rule removed).
2. Launch the runner AMI through `launchRunner`, then `claude --version`, `codex --version`, `ratchet-runner` (exit 2
   without configuration), `aws sts get-caller-identity` with the instance profile.

## Alternative: EC2 Image Builder (no Packer)

The scripts do not depend on Packer, so the same images can be produced by EC2 Image Builder when you prefer an
AWS-managed service. Not built or tested here:

1. Upload `infra/images/scripts/` (and, for the runner, the output of `pack-runner.sh` as `dist/`) to a bucket the
   Image Builder instance role can read, under a versioned prefix.
2. Create one component per image (`aws imagebuilder create-component --platform Linux --version <semver> --data file://c.yml`):

   ```yaml
   name: ratchet-bench          # ratchet-runner: use the runner_scripts list from runner.pkr.hcl
   schemaVersion: 1.0
   phases:
     - name: build
       steps:
         - name: Fetch
           action: S3Download
           inputs:
             - source: s3://<bucket>/ratchet-image/<version>/*
               destination: /tmp/ratchet-image/
         - name: Provision
           action: ExecuteBash
           inputs:
             commands:
               - for s in 00-base 20-aws-cli 25-docker 35-candidate-launcher 70-bench-user 90-verify-bench 99-cleanup; do bash /tmp/ratchet-image/scripts/$s.sh; done
   ```

   (For the runner add `RATCHET_RUNNER_DIST=/tmp/ratchet-image/dist` and the steps from `runner_scripts`; note the
   scripts expect `lib.sh` and `versions*.sh` next to them, which `scripts/` already provides.)
3. Create an image recipe on the Amazon Linux 2023 parent image (x86_64 or arm64) with that component, a root volume of
   30 GB (runner) or 16 GB (bench), and a distribution configuration whose `amiTags` are the table above
   (`ratchet:image=runner|bench`, `ratchet:image_version`), then an image pipeline or `aws imagebuilder create-image`.
4. Read the AMI id from `aws imagebuilder get-image --image-build-version-arn ... --query image.outputResources.amis`
   and pass it as `runnerImageRef` / `benchImageRef`.

Image Builder's built-in `ExecuteBash` runs as root, which the scripts require. Note that its Inspector scan and
test phases are extra; the `90-verify-*` scripts already fail the build on a bad image.

## Known gaps (not fixed here: other packages)

- Fixed since: `RATCHET_SECRET_REFS` is now written comma-separated (what `parseSecretRefs` reads), and the proxy
  variables, including `NODE_USE_ENV_PROXY=1` for Node's fetch, now reach the agent CLIs.
- The bench image has no toolchain and no baked `node_modules`/pnpm store: both come from the repo's container and the dependency
  snapshot (`docs/real-env/bench.md`).
- The Docker, launcher and container-probe parts are tested with shims and a locally compiled launcher only; the AL2023 `docker`
  package contents (`docker-init`, the socket mode) are asserted by `90-verify-bench.sh` at build time.
- Nothing here has run on AWS: the Packer templates are validated, the install steps were run in a scratch prefix, and
  the `dnf`/`useradd`/`iptables`/`systemctl` parts are untested.
