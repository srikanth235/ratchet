import { IMAGE_PREFIX, type ImageSpec } from './image.js';
import { BenchError } from './errors.js';
import { assertSafeRef, BENCH_USER, HOLDOUT_USER, sq, SENTINEL } from './shell.js';

/**
 * Shell for the repo's container (`environment.image`, see image.ts):
 *
 *  - `imageBuildScript`: the IMAGE JOB. Runs through `CloudCompute.runCommand` on an instance launched with purpose `deps` (agent
 *    subnet, egress, an identity that can write `bench-images/` and nothing else). It downloads the staged BASE tree, builds the
 *    image from the protected ref only, saves it (`docker save | zstd`), uploads the archive and THEN the manifest.
 *  - `imageRestoreFragment`: part of the bench checkout script. Root loads the saved image OFFLINE after verifying its sha256 and
 *    its image id, then probes the container exactly as the steps will run it.
 *  - `containerRun`: the `docker run` command line of a bench step. Setup, guardrails and the evaluator run in the container with
 *    `--network none`.
 *
 * Isolation, host path vs container path (the same two uids in both):
 *
 *   property                           host (no `environment.image`)        container
 *   --------------------------------   ----------------------------------   --------------------------------------------
 *   candidate uid                      ratchet-bench via setpriv            same numeric uid via `--user`
 *   harness uid / holdout owner        ratchet-holdout via setpriv          same numeric uid via `--user`
 *   candidate no_new_privs             setpriv --no-new-privs               --security-opt no-new-privileges (and the launcher)
 *   harness -> candidate               sudo rule + launcher script          setuid-root static launcher (CANDIDATE_HELPER) bind-
 *                                                                           mounted read-only; allowed caller / target in a
 *                                                                           root-owned file; bounding set {SETUID, SETGID}
 *   network                            none (no route; endpoints only)     --network none: loopback only, not even the endpoints
 *   instance metadata                  iptables owner rule + hop limit 1    unreachable: no interface; the owner rule still applies
 *   process visibility                 hidepid=2 on /proc                   own PID namespace: the build's processes are invisible;
 *                                                                           harness and candidate share it (different uids, no ptrace)
 *   capabilities                       the users have none                  --cap-drop ALL (harness: only SETUID, SETGID for the launcher)
 *   filesystem                         whole instance disk                  read-only root, tmpfs /tmp, only the checkout, homes and
 *                                                                           out dir bind-mounted; the holdout directory only into
 *                                                                           the confirm harness, read-only
 *   docker socket                      n/a                                  never mounted; the users are not in group docker
 *   seccomp                            n/a                                  Docker's default profile (checked in `docker info`)
 */

/** Setuid-root launcher the harness uses INSIDE the container to start candidate processes (the Rust `ratchet-candidate-launcher` of crates/ratchet-bench, installed by infra/images/scripts/35-candidate-launcher.sh). */
export const CANDIDATE_HELPER = '/usr/local/libexec/ratchet/ratchet-candidate-ctr';
/** Where the container sees it, and the root-owned file that says who may call it and who it becomes. */
export const HELPER_IN_CONTAINER = '/ratchet/candidate';
export const HELPER_CONF_IN_CONTAINER = '/ratchet/launcher.conf';

export const CONTAINER_RESTORE_EXIT = { daemon: 60, seccomp: 61, download: 62, digest: 63, load: 64, imageId: 65, groups: 66, probe: 67, probeHarness: 68, helper: 69 } as const;
export const IMAGE_BUILD_EXIT = { prepare: 10, download: 20, digest: 21, unpack: 22, daemon: 70, build: 71, inspect: 72, pack: 73, uploadArchive: 74, uploadManifest: 75 } as const;

export interface ContainerRuntime {
  /** Tag of the loaded image (`ratchet-env:<key>`). */
  tag: string;
  /** `--read-only` root filesystem (default true). Switch off for toolchains that insist on writing outside the mounted directories. */
  readOnlyRoot?: boolean;
  /** `--pids-limit` (default 4096). */
  pidsLimit?: number;
  /** Size of the tmpfs on /tmp (default 2g). */
  tmpfsSize?: string;
  /** Host path of the setuid launcher. Default `CANDIDATE_HELPER`. Test seam. */
  helperPath?: string;
  /** Verify that the helper is a setuid root-owned file (default true). Only a test on a developer machine turns it off. */
  verifyHelperOwner?: boolean;
  /** How many times (2 s apart) the checkout waits for the docker daemon to answer (default 30). */
  daemonWaitAttempts?: number;
}

const rtOpts = (r: ContainerRuntime) => ({
  readOnlyRoot: r.readOnlyRoot ?? true,
  pidsLimit: r.pidsLimit ?? 4096,
  tmpfsSize: r.tmpfsSize ?? '2g',
  helper: r.helperPath ?? CANDIDATE_HELPER,
  verifyOwner: r.verifyHelperOwner ?? true,
  daemonAttempts: r.daemonWaitAttempts ?? 30,
});

const WORK_RE = /^\/[A-Za-z0-9._\/-]+$/;
const TAG_RE = /^ratchet-env:img-[0-9a-f]{40}$/;
const SIZE_RE = /^[0-9]{1,6}[kmg]$/;

export function assertContainerRuntime(r: ContainerRuntime, work: string): void {
  if (!TAG_RE.test(r.tag)) throw new BenchError(`container tag ${JSON.stringify(r.tag)} is not a ratchet-env tag`);
  if (!WORK_RE.test(work) || work.includes('..')) throw new BenchError(`the work directory ${JSON.stringify(work)} must be a plain absolute path to be bind-mounted into the container`);
  const o = rtOpts(r);
  if (!SIZE_RE.test(o.tmpfsSize)) throw new BenchError(`tmpfs size ${JSON.stringify(o.tmpfsSize)} is not like 512m or 2g`);
  if (!Number.isInteger(o.pidsLimit) || o.pidsLimit < 64) throw new BenchError('pidsLimit must be an integer >= 64');
  if (!WORK_RE.test(o.helper)) throw new BenchError(`helper path ${JSON.stringify(o.helper)} is not a plain absolute path`);
}

/** The fixed shell that runs a command in the container: bash when the image has it, else sh. The command is the last argument. */
const STEP_SHELL = 'if command -v bash >/dev/null 2>&1; then exec bash -c "$1"; else exec sh -c "$1"; fi';

export interface ContainerRunSpec {
  rt: ContainerRuntime;
  /** Shell expression of the container name (a variable, e.g. `"$CN"`). */
  name: string;
  /** Who runs inside: shell expression `uid:gid`. */
  user: string;
  /** Capabilities added back after `--cap-drop ALL`. */
  capAdd?: readonly string[];
  /** `--security-opt no-new-privileges` (false only for the harness: its launcher is setuid). */
  noNewPrivileges: boolean;
  /** `none` for everything on the bench; the image job's package fetch needs the default bridge. */
  network: 'none' | 'bridge';
  /** Shell expression of the working directory. */
  workdir: string;
  /** `src:dst[:ro]`, shell expressions, double quoted by this function. */
  mounts: readonly string[];
  /** Variables whose VALUE is already exported in the script (`-e NAME`). */
  passEnv?: readonly string[];
  /** Variables with values: `NAME=shell expression` (double quoted). */
  env?: readonly string[];
  /** Extra raw words (already valid shell), e.g. `"${DENV[@]}"`. */
  extra?: readonly string[];
  /** The command line for the container's shell (a shell expression: quoted by the caller). */
  command: string;
}

/** `docker run ...` as one line. Everything security-relevant is spelled out here, in one place, and probed after the image is loaded. */
export function containerRun(s: ContainerRunSpec): string {
  const o = rtOpts(s.rt);
  const w: string[] = [
    'docker run --rm',
    `--name ${s.name}`,
    '--pull never', // the bench has no network: never try to pull
    '--init',
    s.network === 'none' ? '--network none' : '',
    '--cap-drop ALL',
    ...(s.capAdd ?? []).map((c) => `--cap-add ${c}`),
    s.noNewPrivileges ? '--security-opt no-new-privileges' : '',
    // seccomp: Docker's default profile (no --security-opt seccomp=unconfined anywhere; checked against `docker info` at restore)
    o.readOnlyRoot ? '--read-only' : '',
    `--tmpfs /tmp:rw,exec,nosuid,nodev,size=${o.tmpfsSize}`,
    `--pids-limit ${o.pidsLimit}`,
    `--user ${s.user}`,
    `--workdir ${s.workdir}`,
    ...s.mounts.map((m) => `-v "${m}"`),
    ...(s.passEnv ?? []).map((e) => `-e ${e}`),
    ...(s.env ?? []).map((e) => `-e "${e}"`),
    ...(s.extra ?? []),
    '--entrypoint sh',
    sq(s.rt.tag),
    `-c ${sq(STEP_SHELL)} ratchet-step ${s.command}`,
  ];
  return w.filter(Boolean).join(' ');
}

/** The ids the container runs as, resolved on the instance. In `none` isolation (tests on a developer machine) the current user. */
export function idsFragment(isolated: boolean): string {
  return isolated
    ? `BUID=$(id -u "$BU"); BGID=$(id -g "$BU"); HUID=$(id -u "$HU"); HGID=$(id -g "$HU")
`
    : `BUID=$(id -u); BGID=$(id -g); HUID=$BUID; HGID=$BGID
`;
}

/** Root-owned file the launcher reads: caller uid, target uid, target gid. Written before every harness container. */
export const launcherConfFragment = (): string => `printf '%s %s %s\\n' "$HUID" "$BUID" "$BGID" > "$W/launcher.conf" && chown root:root "$W/launcher.conf" 2>/dev/null; chmod 644 "$W/launcher.conf" || exit ${CONTAINER_RESTORE_EXIT.helper}
`;

/** Mounts of the harness container for the helper. */
export const helperMounts = (rt: ContainerRuntime): string[] => [`${rtOpts(rt).helper}:${HELPER_IN_CONTAINER}:ro`, `$W/launcher.conf:${HELPER_CONF_IN_CONTAINER}:ro`];

// ---- restore (bench / confirm checkout) -------------------------------------------------------------------------------------

/** A saved image to load before setup (see image.ts `ImageManifest`). */
export interface ImageRestore {
  /** `s3://<artifacts bucket>/bench-images/<key>/<sha256>.<ext>` */
  uri: string;
  sha256: string;
  compression: 'zstd' | 'gzip';
  /** `sha256:<64 hex>`: what `docker image inspect` must report after `docker load`. */
  imageId: string;
  rt: ContainerRuntime;
}

function assertRestore(r: ImageRestore): void {
  if (!/^s3:\/\/[a-z0-9][a-z0-9.-]*\/bench-images\/[A-Za-z0-9._\/-]+$/.test(r.uri) || r.uri.includes('..')) throw new BenchError(`image uri ${JSON.stringify(r.uri)} is not a plain s3:// uri under bench-images/`);
  if (!/^[0-9a-f]{64}$/.test(r.sha256)) throw new BenchError('image digest is not a sha256 hex string');
  if (!/^sha256:[0-9a-f]{64}$/.test(r.imageId)) throw new BenchError('image id is not sha256:<hex>');
}

const daemonReady = (code: number, attempts = 30): string => `command -v docker >/dev/null 2>&1 || { echo "docker is missing on this image (rebuild the bench image)" >&2; exit ${code}; }
for _i in $(seq 1 ${Math.max(1, Math.floor(attempts))}); do docker info >/dev/null 2>&1 && break; sleep 2; done
docker info >/dev/null 2>&1 || { echo "the docker daemon is not running" >&2; exit ${code}; }
`;

/** Download, verify and `docker load` (also used by the image job when an image exists but the dependency snapshot does not). */
function loadImage(r: ImageRestore, dir: string): string {
  const extract = r.compression === 'zstd' ? `zstd -dc -- "$IMG/in/image" | docker load` : `gzip -dc -- "$IMG/in/image" | docker load`;
  return `IMG=${dir}; mkdir -p "$IMG/in" || exit ${CONTAINER_RESTORE_EXIT.download}
aws s3 cp --only-show-errors ${sq(r.uri)} "$IMG/in/image" || { echo "cannot download the container image ${r.uri}" >&2; exit ${CONTAINER_RESTORE_EXIT.download}; }
[ "$(sha256sum -- "$IMG/in/image" | cut -d' ' -f1)" = ${sq(r.sha256)} ] || { echo "sha256 mismatch for the container image" >&2; exit ${CONTAINER_RESTORE_EXIT.digest}; }
${extract} >"$IMG/load.out" 2>&1 || { tail -c 600 "$IMG/load.out" >&2; echo "docker load failed" >&2; exit ${CONTAINER_RESTORE_EXIT.load}; }
rm -f "$IMG/in/image"
[ "$(docker image inspect --format '{{.Id}}' ${sq(r.rt.tag)} 2>/dev/null)" = ${sq(r.imageId)} ] || { echo "the loaded image is not ${r.imageId} (tag ${r.rt.tag})" >&2; exit ${CONTAINER_RESTORE_EXIT.imageId}; }
`;
}

/** Runs a short command in the loaded image the way a setup step would, but as root-free, offline and read-only: `ctr 'node --version'`. */
export const ctrFunction = (rt: ContainerRuntime): string =>
  `ctr() { docker run --rm --pull never --network none --cap-drop ALL --security-opt no-new-privileges --read-only --user "$BUID:$BGID" --entrypoint sh ${sq(rt.tag)} -c "$1"; }
`;

const probeSetup = `[ "$(id -u)" = "$EXPECT_UID" ] || { echo "runs as uid $(id -u), expected $EXPECT_UID"; exit 1; }
{ [ ! -e /var/run/docker.sock ] && [ ! -e /run/docker.sock ]; } || { echo "a docker socket is visible"; exit 1; }
grep -Eq '^CapEff:[[:space:]]*0+$' /proc/self/status || { echo "capabilities are not dropped"; exit 1; }
grep -Eq '^NoNewPrivs:[[:space:]]*1' /proc/self/status || { echo "no_new_privs is not set"; exit 1; }
n=$(ls /sys/class/net 2>/dev/null | tr '\\n' ' '); [ "$n" = "lo " ] || [ -z "$n" ] || { echo "network interfaces visible: $n"; exit 1; }
echo ok`;

const probeRootfs = `touch /ratchet-rootfs-probe 2>/dev/null && { echo "the root filesystem is writable"; exit 1; }; echo ok`;

const probeHarness = `[ "$(id -u)" = "$HARNESS_UID" ] || { echo "harness runs as uid $(id -u), expected $HARNESS_UID"; exit 1; }
cat "$PROBE_FILE" >/dev/null 2>&1 || { echo "the harness cannot read the holdout directory"; exit 1; }
out=$(${HELPER_IN_CONTAINER} sh -c 'id -u; grep -E "^NoNewPrivs" /proc/self/status; if cat "$1" >/dev/null 2>&1; then echo LEAK; fi' x "$PROBE_FILE" 2>&1) || { echo "the launcher failed: $out"; exit 1; }
[ "$(printf '%s\\n' "$out" | sed -n 1p)" = "$EXPECT_UID" ] || { echo "the launcher did not run the candidate as uid $EXPECT_UID: $out"; exit 1; }
printf '%s\\n' "$out" | grep -Eq 'NoNewPrivs:[[:space:]]*1' || { echo "candidate processes have no no_new_privs"; exit 1; }
printf '%s\\n' "$out" | grep -q LEAK && { echo "a candidate process can read the holdout"; exit 1; }
echo ok`;

/**
 * Root, in the bench checkout, after the isolation checks: load the saved image offline and prove the container is what the steps
 * assume. Exit 60 to 69.
 */
export function imageRestoreFragment(r: ImageRestore, o: { work: string; isolated: boolean }): string {
  assertRestore(r);
  assertContainerRuntime(r.rt, o.work);
  const rt = rtOpts(r.rt);
  const helperCheck = o.isolated
    ? `[ -x ${sq(rt.helper)} ] && [ -u ${sq(rt.helper)} ]${rt.verifyOwner ? ` && [ "$(stat -c %u ${sq(rt.helper)})" = 0 ]` : ''} || { echo "the candidate launcher ${rt.helper} is missing or is not a setuid root file (rebuild the bench image)" >&2; exit ${CONTAINER_RESTORE_EXIT.helper}; }
`
    : '';
  const groups = o.isolated
    ? `for u in "$BU" "$HU"; do
  if id -nG "$u" 2>/dev/null | tr ' ' '\\n' | grep -qxE 'docker|root|wheel|sudo'; then echo "$u is in a privileged group (docker/root/wheel/sudo): it could reach the docker socket" >&2; exit ${CONTAINER_RESTORE_EXIT.groups}; fi
done
`
    : '';
  const probeRun = (script: string): string =>
    containerRun({
      rt: r.rt,
      name: '"$CN"',
      user: '"$BUID:$BGID"',
      noNewPrivileges: true,
      network: 'none',
      workdir: '/',
      mounts: [],
      command: `"EXPECT_UID=$BUID;"${sq(script)}`,
    });
  const harnessProbeRun = containerRun({
    rt: r.rt,
    name: '"$CN"',
    user: '"$HUID:$HGID"',
    capAdd: ['SETUID', 'SETGID'],
    noNewPrivileges: false,
    network: 'none',
    workdir: '/',
    mounts: [`$W/holdout:$W/holdout:ro`, ...helperMounts(r.rt)],
    env: ['RATCHET_CANDIDATE_DIR=/', 'RATCHET_CANDIDATE_HOME=/tmp', 'RATCHET_CANDIDATE_TMP=/tmp'],
    command: `"HARNESS_UID=$HUID; EXPECT_UID=$BUID; PROBE_FILE=$W/holdout/.cprobe;"${sq(probeHarness)}`,
  });
  return `# ---- container image (root): loaded offline, verified, probed ----
${daemonReady(CONTAINER_RESTORE_EXIT.daemon, rt.daemonAttempts)}docker info --format '{{json .SecurityOptions}}' 2>/dev/null | grep -q seccomp || { echo "the docker daemon runs without its default seccomp profile" >&2; exit ${CONTAINER_RESTORE_EXIT.seccomp}; }
${idsFragment(o.isolated)}${groups}${loadImage(r, '"$W/image"')}${ctrFunction(r.rt)}CN="ratchet-$(basename -- "$W")-probe"
${helperCheck}${
    o.isolated
      ? `${launcherConfFragment()}printf x > "$W/holdout/.cprobe" && chown "$HU:" "$W/holdout/.cprobe" && chmod 600 "$W/holdout/.cprobe" || exit ${CONTAINER_RESTORE_EXIT.probe}
`
      : ''
  }docker rm -f "$CN" >/dev/null 2>&1 || true
pr=$(${probeRun(probeSetup)} 2>&1) || { echo "container probe failed: $pr" >&2; exit ${CONTAINER_RESTORE_EXIT.probe}; }
${
    rt.readOnlyRoot
      ? `pr=$(${probeRun(probeRootfs)} 2>&1) || { echo "container probe failed: $pr" >&2; exit ${CONTAINER_RESTORE_EXIT.probe}; }
`
      : ''
  }${
    o.isolated
      ? `docker rm -f "$CN" >/dev/null 2>&1 || true
pr=$(${harnessProbeRun} 2>&1) || { echo "harness container probe failed: $pr" >&2; rm -f "$W/holdout/.cprobe"; exit ${CONTAINER_RESTORE_EXIT.probeHarness}; }
rm -f "$W/holdout/.cprobe"
`
      : ''
  }emit container "{\\"tag\\":\\"${r.rt.tag}\\",\\"imageId\\":\\"${r.imageId}\\",\\"readOnlyRoot\\":${rt.readOnlyRoot},\\"network\\":\\"none\\"}"
`;
}

/** Same as `imageRestoreFragment`'s load, for the deps job on an instance that may already hold the image: a no-op when it is loaded. */
export function ensureImageLoaded(r: ImageRestore): string {
  assertRestore(r);
  return `${daemonReady(IMAGE_BUILD_EXIT.daemon)}if [ "$(docker image inspect --format '{{.Id}}' ${sq(r.rt.tag)} 2>/dev/null)" != ${sq(r.imageId)} ]; then
${loadImage(r, '"$W/image"')}fi
`;
}

// ---- the image job ------------------------------------------------------------------------------------------------------------

export interface ImageBuildInput {
  work: string;
  /** The staged BASE tree (`deps-base.tgz`). */
  base: { uri: string; sha256: string; commit: string };
  key: string;
  tag: string;
  spec: ImageSpec;
  /** Hashes of the environment files as the controller computed them. */
  files: Readonly<Record<string, string>>;
  arch: string;
  bucket: string;
  /** Limit for the build. */
  timeoutMs: number;
}

const BUCKET_RE = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/;

/**
 * The image job, one script: download the staged BASE tree (never the candidate), build or pull the image with the daemon of this
 * throwaway instance, save it, upload the archive and then the manifest. `docker build` runs as root on an instance that has no
 * candidate code: the Dockerfile is protected-ref input (as trusted as the lockfile), its RUN steps run in build containers whose
 * route to instance metadata is blocked by the user data (FORWARD / DOCKER-USER rule, hop limit 1) and which hold no credentials
 * (DOCKER_CONFIG points at an empty directory plus the proxy settings; AWS_* are not in the environment).
 */
export function imageBuildScript(o: ImageBuildInput): string {
  if (!/^img-[0-9a-f]{40}$/.test(o.key)) throw new BenchError(`image key ${JSON.stringify(o.key)} is not a plain key`);
  if (o.tag !== `ratchet-env:${o.key}`) throw new BenchError('image tag does not match its key');
  if (!BUCKET_RE.test(o.bucket)) throw new BenchError(`artifacts bucket ${JSON.stringify(o.bucket)} is not a valid bucket name`);
  if (!WORK_RE.test(o.work)) throw new BenchError(`work directory ${JSON.stringify(o.work)} is not a plain absolute path`);
  assertSafeRef(o.base.commit, 'base commit');
  if (!/^s3:\/\/[a-z0-9][a-z0-9.-]*\/[A-Za-z0-9._\/-]+\/deps-base\.tgz$/.test(o.base.uri) || o.base.uri.includes('..')) {
    throw new BenchError(`staged base tree uri ${JSON.stringify(o.base.uri)} is not a plain s3:// uri ending in deps-base.tgz`);
  }
  if (!/^[0-9a-f]{64}$/.test(o.base.sha256)) throw new BenchError('staged base tree digest is not a sha256 hex string');
  const s = o.spec;
  const timeoutS = Math.max(1, Math.ceil(o.timeoutMs / 1000));
  const rel = (p: string) => (/^[A-Za-z0-9._\/ -]+$/.test(p) && !p.includes('..') ? p : (() => { throw new BenchError(`path ${JSON.stringify(p)} is not plain`); })());
  let build: string;
  if (s.kind === 'dockerfile') {
    const args = Object.entries(s.args)
      .map(([k, v]) => `--build-arg ${sq(`${k}=${v}`)}`)
      .join(' ');
    // The build runs in `env -i bash -c`: absolute paths, no shell variables of this script.
    build = `docker build --pull --force-rm --tag ${sq(o.tag)} --file ${sq(`${o.work}/tree/${rel(s.dockerfile!)}`)} ${s.target ? `--target ${sq(s.target)} ` : ''}${args} ${sq(`${o.work}/tree/${rel(s.context!)}`)}`;
  } else {
    build = `docker pull ${sq(s.image!)} && docker tag ${sq(s.image!)} ${sq(o.tag)}`;
  }
  const head = JSON.stringify({ v: 1, key: o.key, rev: 1, kind: s.kind, path: s.path, arch: o.arch, files: o.files, baseDigests: s.baseDigests, tag: o.tag });
  const archivePrefix = `${IMAGE_PREFIX}${o.key}`;

  return `set -uo pipefail
W=${sq(o.work)}
TREE="$W/tree"; OUT="$W/out"
emit() { printf '%s %s %s\\n' '${SENTINEL}' "$1" "$2"; }
rm -rf -- "\${W:?}"; mkdir -p "$TREE" "$OUT" "$W/in" "$W/home" "$W/docker-config" || exit ${IMAGE_BUILD_EXIT.prepare}
chmod 755 "$W"
aws s3 cp --only-show-errors ${sq(o.base.uri)} "$W/in/base.tgz" || { echo "cannot download the base tree ${o.base.uri}" >&2; exit ${IMAGE_BUILD_EXIT.download}; }
[ "$(sha256sum -- "$W/in/base.tgz" | cut -d' ' -f1)" = ${sq(o.base.sha256)} ] || { echo "sha256 mismatch for the base tree" >&2; exit ${IMAGE_BUILD_EXIT.digest}; }
tar -xzf "$W/in/base.tgz" -C "$TREE" --no-same-owner || exit ${IMAGE_BUILD_EXIT.unpack}
rm -f "$W/in/base.tgz"
${daemonReady(IMAGE_BUILD_EXIT.daemon)}[ -r /etc/ratchet/runner.env ] && { set -a; . /etc/ratchet/runner.env; set +a; }
# Proxy settings reach the build containers through the docker client config (the daemon's own pulls use its systemd drop-in).
if [ -n "\${HTTPS_PROXY:-}" ]; then
  printf '{"proxies":{"default":{"httpProxy":"%s","httpsProxy":"%s","noProxy":"%s"}}}\\n' "\${HTTP_PROXY:-$HTTPS_PROXY}" "$HTTPS_PROXY" "\${NO_PROXY:-}" > "$W/docker-config/config.json"
fi
cd "$TREE" || exit ${IMAGE_BUILD_EXIT.prepare}
s=$(date +%s%N)
timeout -k 10 ${timeoutS} env -i PATH="$PATH" HOME="$W/home" DOCKER_CONFIG="$W/docker-config" bash -c ${sq(build)} </dev/null >"$W/build.out" 2>&1; rc=$?
e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
echo "--- build output tail ---"; tail -c 4000 "$W/build.out"
[ $rc -eq 0 ] || { echo "the image build exited with status $rc" >&2; exit ${IMAGE_BUILD_EXIT.build}; }

IMAGEID=$(docker image inspect --format '{{.Id}}' ${sq(o.tag)} 2>/dev/null)
case "$IMAGEID" in sha256:????????????????????????????????????????????????????????????????) ;; *) echo "cannot read the id of the built image" >&2; exit ${IMAGE_BUILD_EXIT.inspect};; esac
if command -v zstd >/dev/null 2>&1; then
  EXT=tar.zst; COMP=zstd
  docker save ${sq(o.tag)} | zstd -T0 -3 -q -o "$OUT/image" || exit ${IMAGE_BUILD_EXIT.pack}
else
  EXT=tar.gz; COMP=gzip
  docker save ${sq(o.tag)} | gzip -3 > "$OUT/image" || exit ${IMAGE_BUILD_EXIT.pack}
fi
SHA=$(sha256sum -- "$OUT/image" | cut -d' ' -f1); BYTES=$(stat -c %s -- "$OUT/image")
ARCHIVE=${sq(archivePrefix)}/"$SHA.$EXT"
printf '%s,"imageId":"%s","compression":"%s","archive":"%s","sha256":"%s","bytes":%s,"createdAt":%s}\\n' ${sq(head.slice(0, -1))} "$IMAGEID" "$COMP" "$ARCHIVE" "$SHA" "$BYTES" "$(date +%s%3N)" > "$OUT/manifest.json"
aws s3 cp --only-show-errors "$OUT/image" "s3://${o.bucket}/$ARCHIVE" || { echo "cannot upload the image" >&2; exit ${IMAGE_BUILD_EXIT.uploadArchive}; }
aws s3 cp --only-show-errors "$OUT/manifest.json" ${sq(`s3://${o.bucket}/${archivePrefix}.json`)} || { echo "cannot upload the manifest" >&2; exit ${IMAGE_BUILD_EXIT.uploadManifest}; }
emit image "{\\"key\\":\\"${o.key}\\",\\"imageId\\":\\"$IMAGEID\\",\\"sha256\\":\\"$SHA\\",\\"bytes\\":$BYTES,\\"compression\\":\\"$COMP\\",\\"ms\\":$ms}"
exit 0
`;
}

export { BENCH_USER, HOLDOUT_USER };
