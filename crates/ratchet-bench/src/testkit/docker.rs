//! A `docker` shim for the fake bench instances (port of `testkit/docker.ts`): enough of the CLI for the generated scripts,
//! with one image store PER INSTANCE (`RATCHET_FAKE_INSTANCE`) so that "the bench instance loaded the image" is something a
//! test can observe.
//!
//!  - `build`   makes an image whose id is the hash of the Dockerfile and the build arguments (so equal inputs give equal images)
//!  - `save`    prints a tiny archive; `load` reads it back; `image inspect` prints the id; `pull`/`tag` copy
//!  - `run`     parses the flags (logged, one line per call, in `log`), refuses an image that was not loaded (`--pull never`)
//!    and executes the command on THIS machine: the bind mounts are at the same paths, the environment is `env -i` plus
//!    the `-e` values, the working directory is `--workdir`. Probe scripts are answered by the shim itself because the
//!    developer machine does not drop capabilities.
//!
//! Flags files in `flags` (touch to enable): `no-seccomp` (docker info lacks seccomp), `no-daemon` (info fails),
//! `fail-build`, `bad-id` (inspect reports another id than the builder saw), `probe-fails` (the setup probe reports a
//! problem), `harness-probe-fails`.

use std::path::Path;

fn q(p: &Path) -> String {
    serde_json::to_string(&p.to_string_lossy()).unwrap_or_default()
}

pub fn docker_shim(state: &Path, log: &Path, flags: &Path) -> String {
    let template = r#"
# Which fake instance is calling: from our environment, else from an ancestor's (the image build runs under `env -i`).
inst="${RATCHET_FAKE_INSTANCE:-}"; p=$$
while [ -z "$inst" ] && [ "$p" -gt 1 ]; do
  inst=$(tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | sed -n 's/^RATCHET_FAKE_INSTANCE=//p')
  p=$(awk '{print $4}' "/proc/$p/stat" 2>/dev/null || echo 1)
done
STATE=@STATE@/"${inst:-i0}"; LOG=@LOG@; FLAGS=@FLAGS@
mkdir -p "$STATE/images"
flag() { [ -e "$FLAGS/$1" ]; }
echo "docker $*" >> "$LOG"
sub="${1:-}"; shift || true
idof() { local f="$STATE/images/$(printf '%s' "$1" | tr '/:' '__')"; [ -f "$f" ] && cat "$f"; }
case "$sub" in
  info)
    flag no-daemon && { echo "Cannot connect to the Docker daemon" >&2; exit 1; }
    if [ "${1:-}" = --format ]; then
      if flag no-seccomp; then echo '["name=cgroupns"]'; else echo '["name=seccomp,profile=builtin","name=cgroupns"]'; fi
    fi
    exit 0;;
  rm) exit 0;;
  build)
    flag fail-build && { echo "ERROR: failed to solve: process did not complete successfully" >&2; exit 1; }
    tag=""; file=""; args=""
    while [ $# -gt 0 ]; do case "$1" in --tag) tag="$2"; shift 2;; --file) file="$2"; shift 2;; --build-arg|--target) args="$args $1=$2"; shift 2;; --pull|--force-rm) shift;; *) shift;; esac; done
    [ -f "$file" ] || { echo "no Dockerfile at $file" >&2; exit 1; }
    id="sha256:$( { cat "$file"; echo "$args"; } | sha256sum | cut -d' ' -f1)"
    printf '%s' "$id" > "$STATE/images/$(printf '%s' "$tag" | tr '/:' '__')"
    echo "Successfully built $id"; exit 0;;
  pull) id="sha256:$(printf '%s' "$1" | sha256sum | cut -d' ' -f1)"; printf '%s' "$id" > "$STATE/images/$(printf '%s' "$1" | tr '/:' '__')"; exit 0;;
  tag) cp "$STATE/images/$(printf '%s' "$1" | tr '/:' '__')" "$STATE/images/$(printf '%s' "$2" | tr '/:' '__')"; exit 0;;
  image)
    # docker image inspect --format '{{.Id}}' TAG
    tag="${@: -1}"
    id="$(idof "$tag")" || exit 1
    flag bad-id && id="sha256:$(printf '%064d' 7)"
    echo "$id"; exit 0;;
  save)
    id="$(idof "$1")" || { echo "no such image $1" >&2; exit 1; }
    printf 'FAKEIMAGE %s %s\n' "$id" "$1"; exit 0;;
  load)
    read -r magic id tag || exit 1
    [ "$magic" = FAKEIMAGE ] || { echo "not an image archive" >&2; exit 1; }
    printf '%s' "$id" > "$STATE/images/$(printf '%s' "$tag" | tr '/:' '__')"
    echo "Loaded image: $tag"; exit 0;;
  run)
    declare -a envs=(); workdir=""; image=""; entry=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --rm|--init|--read-only|--privileged) shift;;
        --entrypoint) entry="$2"; shift 2;;
        --name|--pull|--network|--cap-drop|--cap-add|--security-opt|--tmpfs|--pids-limit|--user|--ulimit|--memory|--cpus) shift 2;;
        --workdir|-w) workdir="$2"; shift 2;;
        -v|--volume|--mount) shift 2;;
        -e|--env) case "$2" in *=*) envs+=("$2");; *) envs+=("$2=${!2-}");; esac; shift 2;;
        -*) echo "docker shim: unknown flag $1" >&2; exit 125;;
        *) image="$1"; shift; break;;
      esac
    done
    [ -n "$(idof "$image")" ] || { echo "Unable to find image '$image' locally" >&2; exit 125; }
    script="${4:-}"
    case "$script" in
      *HARNESS_UID=*)
        flag harness-probe-fails && { echo "a candidate process can read the holdout"; exit 1; }
        echo ok; exit 0;;
      *EXPECT_UID=*"NoNewPrivs"*)
        flag probe-fails && { echo "network interfaces visible: eth0"; exit 1; }
        echo ok; exit 0;;
      *"ratchet-rootfs-probe"*) echo ok; exit 0;;
    esac
    [ -n "$workdir" ] && [ -d "$workdir" ] && cd "$workdir"
    exec env -i PATH="$PATH" "${envs[@]+"${envs[@]}"}" ${entry:+"$entry"} "$@";;
  *) echo "docker shim: unsupported subcommand $sub" >&2; exit 125;;
esac
"#;
    template
        .replace("@STATE@", &q(state))
        .replace("@LOG@", &q(log))
        .replace("@FLAGS@", &q(flags))
}
