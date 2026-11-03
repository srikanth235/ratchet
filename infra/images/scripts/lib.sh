# shellcheck shell=bash
# shellcheck disable=SC2034
# Helpers shared by the provisioning scripts. Sourced, never executed.
#
# Test hook: RATCHET_PREFIX (default /usr/local) moves every install under another
# directory (and RATCHET_ETC another /etc) so the download/verify/extract logic can be
# exercised without root.
# On an image build it is unset.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=versions.sh
. "$SCRIPT_DIR/versions.sh"

PREFIX="${RATCHET_PREFIX:-/usr/local}"
ETC="${RATCHET_ETC:-/etc}"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

require_root() {
  [ -n "${RATCHET_PREFIX:-}" ] && return 0
  [ "$(id -u)" -eq 0 ] || die "must run as root"
}

# Architecture names used by the different vendors for the machine we run on.
case "$(uname -m)" in
  x86_64)
    ARCH_UNAME=x86_64; ARCH_NODE=x64; ARCH_GO=amd64 ;;
  aarch64 | arm64)
    ARCH_UNAME=aarch64; ARCH_NODE=arm64; ARCH_GO=arm64 ;;
  *) die "unsupported architecture $(uname -m)" ;;
esac

# var NAME: value of a variable whose name is built at run time (e.g. NODE_SHA256_x64).
var() {
  local n="$1"
  [ -n "${!n:-}" ] || die "no pinned value for $n in versions.sh"
  printf '%s' "${!n}"
}

# Scratch directory, removed on exit.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# fetch URL DEST: https only, retries, fails on HTTP errors.
fetch() {
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    --retry 5 --retry-delay 3 --retry-all-errors --connect-timeout 20 -o "$2" "$1"
}

# verify_sha256 FILE HEX
verify_sha256() {
  local got
  got="$(sha256sum "$1" | cut -d' ' -f1)"
  [ "$got" = "$2" ] || die "sha256 mismatch for $(basename "$1"): expected $2, got $got"
}

# verify_sri FILE sha512-BASE64 (the "integrity" value of an npm registry tarball)
verify_sri() {
  local want="${2#sha512-}" got
  [ "$want" != "$2" ] || die "unsupported integrity value for $(basename "$1"): $2"
  got="$(openssl dgst -sha512 -binary "$1" | openssl base64 -A)"
  [ "$got" = "$want" ] || die "integrity mismatch for $(basename "$1"): expected $2, got sha512-$got"
}

# fetch_verified URL DEST SHA256HEX
fetch_verified() {
  fetch "$1" "$2"
  verify_sha256 "$2" "$3"
}

# npm_fetch_verified NAME VERSION INTEGRITY: downloads the registry tarball of NAME@VERSION into
# $WORK, checks its SRI digest against the pinned one and prints the tarball path.
npm_fetch_verified() {
  local name="$1" version="$2" integrity="$3" out
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "npm version must be exact, got '$version'"
  out="$(cd "$WORK" && npm pack "${name}@${version}" --silent --pack-destination "$WORK" | tail -n 1)"
  [ -f "$WORK/$out" ] || die "npm pack ${name}@${version} produced no tarball"
  verify_sri "$WORK/$out" "$integrity"
  printf '%s' "$WORK/$out"
}

# dnf wrapper. RATCHET_DNF_ARGS can pin the AL2023 repository snapshot, e.g.
# RATCHET_DNF_ARGS=--releasever=2023.9.20261012 (see the README, "Reproducibility").
dnf_install() {
  # shellcheck disable=SC2086
  dnf install -y --setopt=install_weak_deps=False ${RATCHET_DNF_ARGS:-} "$@"
}

# elf_field FILE OFFSET SIZE: the unsigned integer of SIZE bytes (1, 2, 4 or 8) at byte OFFSET. od reads in host byte
# order; every host these scripts run on (x86_64, aarch64) is little endian, like the ELF files they check.
elf_field() { od -An -v -tu"$3" -j "$2" -N "$3" "$1" | tr -d ' \n'; }

# elf_is_native FILE: a 64-bit little-endian ELF executable or PIE for the machine we run on.
elf_is_native() {
  local want
  case "$ARCH_UNAME" in x86_64) want=62 ;; aarch64) want=183 ;; *) return 1 ;; esac
  [ "$(od -An -v -tx1 -N6 "$1" | tr -d ' \n')" = "7f454c460201" ] || return 1
  case "$(elf_field "$1" 16 2)" in 2 | 3) ;; *) return 1 ;; esac # ET_EXEC | ET_DYN
  [ "$(elf_field "$1" 18 2)" = "$want" ]
}

# elf_is_static FILE: no program interpreter (PT_INTERP). A static-pie executable (what Rust's crt-static produces for a
# glibc target) makes glibc's `ldd` print "statically linked" and exit 0, so ldd's status cannot tell static from dynamic.
elf_is_static() {
  local phoff phentsize phnum i
  phoff="$(elf_field "$1" 32 8)"
  phentsize="$(elf_field "$1" 54 2)"
  phnum="$(elf_field "$1" 56 2)"
  { [ "${phnum:-0}" -gt 0 ] && [ "${phentsize:-0}" -ge 56 ]; } || return 1
  for ((i = 0; i < phnum; i++)); do
    [ "$(elf_field "$1" $((phoff + i * phentsize)) 4)" != 3 ] || return 1
  done
}

# link_into_bin TARGET NAME: symlink in $PREFIX/bin
link_into_bin() {
  mkdir -p "$PREFIX/bin"
  ln -sfn "$1" "$PREFIX/bin/$2"
}

# Everything we install must be readable and executable by every user: the bench runs candidate
# code as the unprivileged ratchet-bench user, and nothing may live under /root.
make_world_readable() { chmod -R a+rX "$@"; }

# in_group USER GROUP
in_group() {
  case " $(id -nG "$1") " in *" $2 "*) return 0 ;; esac
  return 1
}
