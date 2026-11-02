#!/usr/bin/env bash
# Regenerate the `real` agent-CLI fixtures from the CLIs installed on this machine.
#
#   scripts/record-fixtures.sh [--cli claude|codex|grok|all] [--behaviour LIST] [--out DIR] [--yes] [--json]
#
# Without --yes it only reports what it would record and where (nothing is run, nothing is written).
# Run `scripts/record-fixtures.sh --help` for the rest.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/crates/ratchet-agents/tests/fixtures"
CLIS="all"
BEHAVIOURS="success,deltas,resume,auth-expired"
YES=0
JSON=0
TIMEOUT_S="${RECORD_TIMEOUT_S:-120}"
PROMPT="Reply with exactly the word pong and nothing else. Do not use any tools."

usage() {
  cat <<'END_OF_USAGE'
record-fixtures.sh: record `real` fixtures from the installed agent CLIs.

SYNOPSIS
  scripts/record-fixtures.sh [options]

OPTIONS
  --cli NAME         claude, codex, grok, or all (default: all). `all` skips CLIs that are not installed;
                     naming one that is not installed is an error.
  --behaviour LIST   comma list of: success, deltas, resume, auth-expired (default: all four).
  --out DIR          where fixtures go (default: crates/ratchet-agents/tests/fixtures).
  --yes              record and write. Without it nothing runs and nothing is written (a plan is printed).
  --json             print the result as one JSON array on stdout (status messages go to stderr).
  -h, --help         this text.

WHAT IT DOES
  For each CLI it finds on PATH it reads `<cli> --version`, runs the CLI once per behaviour in a scratch directory,
  scrubs the output, and writes  <cli>-<version>-real-<behaviour>.jsonl  into --out. The file name is the
  scheme `tests/real_fixtures.rs` enforces; `real` means the CLI itself produced it.

  success        one short turn with no tools        (claude -p, codex exec --json, grok -p, as the adapters run them)
  deltas         partial text                        (claude --include-partial-messages; grok streams by default;
                                                      `codex exec` has no partial text, so codex is skipped)
  resume         a second turn on the first one's session id
  auth-expired   the same call with a rejected key and an empty CLI home (no login is read or changed)
  Rate limits cannot be recorded on demand; their fixtures stay `mock` until one is captured by hand.

SECRETS
  Nothing secret is stored. The CLI's stderr is dropped. Values of environment variables whose names contain
  KEY, TOKEN, SECRET, PASSWORD or AUTH, your home directory, the scratch directory and your user name are replaced
  before anything is written. If a credential-looking string is still in the output, the recording is deleted and
  the script exits 4.

EXIT CODES
  0 recorded (or planned)   2 bad arguments   3 a named CLI is not installed   4 recording failed or leaked a secret

EXAMPLES
  scripts/record-fixtures.sh                          # plan only
  scripts/record-fixtures.sh --cli claude --yes       # re-record the Claude Code fixtures
  scripts/record-fixtures.sh --behaviour success --yes --json
  ANTHROPIC_API_KEY=... scripts/record-fixtures.sh --cli claude --yes   # key-based, as the canary runs it
END_OF_USAGE
}

die() { # die CODE MESSAGE
  local code="$1"; shift
  echo "record-fixtures: $*" >&2
  exit "$code"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --cli) [ $# -ge 2 ] || die 2 "--cli needs a value (claude, codex, grok or all). See --help."; CLIS="$2"; shift 2 ;;
    --behaviour) [ $# -ge 2 ] || die 2 "--behaviour needs a comma list. See --help."; BEHAVIOURS="$2"; shift 2 ;;
    --out) [ $# -ge 2 ] || die 2 "--out needs a directory."; OUT="$2"; shift 2 ;;
    --yes) YES=1; shift ;;
    --json) JSON=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die 2 "unknown argument '$1'. See --help." ;;
  esac
done

case "$CLIS" in
  all) LIST="claude codex grok" ;;
  claude|codex|grok) LIST="$CLIS" ;;
  *) die 2 "unknown --cli '$CLIS'. Use claude, codex, grok or all." ;;
esac
IFS=',' read -r -a BEHAVIOUR_LIST <<<"$BEHAVIOURS"
for b in "${BEHAVIOUR_LIST[@]}"; do
  case "$b" in
    success|deltas|resume|auth-expired) ;;
    *) die 2 "unknown behaviour '$b'. Use success, deltas, resume, auth-expired." ;;
  esac
done

say() { echo "$@" >&2; }

RESULTS=()
record_result() { # cli version behaviour file status [note]
  local note="${6:-}"
  RESULTS+=("{\"cli\":\"$1\",\"version\":\"$2\",\"behaviour\":\"$3\",\"file\":\"$4\",\"status\":\"$5\",\"note\":\"$note\"}")
}

# ---- version detection ---------------------------------------------------------------------------------------------
version_of() { # cli -> x.y.z on stdout, empty when it cannot be read
  local out
  out="$("$1" --version 2>/dev/null | head -n 1 || true)"
  printf '%s' "$out" | grep -Eo '[0-9]+\.[0-9]+\.[0-9]+' | head -n 1 || true
}

# ---- scrubbing -----------------------------------------------------------------------------------------------------
# Replace secrets and machine-specific strings. Reads stdin, writes stdout.
scrub() { # scratch-dir
  local scratch="$1" expr=() name value
  while IFS='=' read -r name value; do
    case "$name" in
      *KEY*|*TOKEN*|*SECRET*|*PASSWORD*|*AUTH*)
        if [ "${#value}" -ge 8 ]; then
          expr+=(-e "s|$(printf '%s' "$value" | sed 's/[][\.*^$|/&]/\\&/g')|REDACTED|g")
        fi ;;
    esac
  done < <(env)
  expr+=(-e "s|$(printf '%s' "$scratch" | sed 's/[][\.*^$|/&]/\\&/g')|/work/recording|g")
  if [ -n "${HOME:-}" ] && [ "$HOME" != "/" ]; then
    expr+=(-e "s|$(printf '%s' "$HOME" | sed 's/[][\.*^$|/&]/\\&/g')|/home/recorder|g")
  fi
  local user="${USER:-$(id -un 2>/dev/null || true)}"
  if [ "${#user}" -ge 3 ]; then
    expr+=(-e "s|/$user/|/recorder/|g")
  fi
  sed "${expr[@]}"
}

# Credential-looking strings that must never reach a fixture. Keep in step with `secret_in` in tests/real_fixtures.rs.
leaks() { # file -> prints the first match, returns 0 when there is one
  grep -E -o -m1 'sk-ant-|sk-proj-|xai-|ghp_|github_pat_|AKIA|Bearer |-----BEGIN|api_key":"|apiKey":"|access_token":"|refresh_token":"' "$1" | head -n 1 | grep . 
}

first_match() { # pattern file
  grep -Eo "$1" "$2" | head -n 1 | sed -E 's/.*:"([^"]*)"$/\1/' || true
}

# ---- one recording -------------------------------------------------------------------------------------------------
# run_cli CLI BEHAVIOUR SCRATCH [SESSION_ID] -> raw stdout in the file $SCRATCH/raw.jsonl; returns the CLI's exit code
run_cli() {
  local cli="$1" behaviour="$2" scratch="$3" session="${4:-}" code=0
  local env_prefix=()
  if [ "$behaviour" = auth-expired ]; then
    mkdir -p "$scratch/empty-home"
    case "$cli" in
      claude) env_prefix=(env HOME="$scratch/empty-home" CLAUDE_CONFIG_DIR="$scratch/empty-home" ANTHROPIC_API_KEY="invalid-key-for-recording") ;;
      codex) env_prefix=(env HOME="$scratch/empty-home" CODEX_HOME="$scratch/empty-home" OPENAI_API_KEY="invalid-key-for-recording" CODEX_API_KEY="invalid-key-for-recording") ;;
      grok) env_prefix=(env HOME="$scratch/empty-home" GROK_HOME="$scratch/empty-home" XAI_API_KEY="invalid-key-for-recording" GROK_API_KEY="invalid-key-for-recording") ;;
    esac
  fi
  local args=()
  case "$cli" in
    claude)
      args=(-p --output-format stream-json --verbose --max-turns 1 --permission-mode dontAsk)
      [ "$behaviour" = deltas ] && args+=(--include-partial-messages)
      [ -n "$session" ] && args+=(--resume "$session")
      printf '%s' "$PROMPT" | (cd "$scratch" && timeout "$TIMEOUT_S" "${env_prefix[@]}" claude "${args[@]}") >"$scratch/raw.jsonl" 2>/dev/null || code=$?
      ;;
    codex)
      if [ -n "$session" ]; then
        args=(exec resume --json --skip-git-repo-check "$session")
      else
        args=(exec --json --skip-git-repo-check --sandbox read-only)
      fi
      printf '%s' "$PROMPT" | (cd "$scratch" && timeout "$TIMEOUT_S" "${env_prefix[@]}" codex "${args[@]}") >"$scratch/raw.jsonl" 2>/dev/null || code=$?
      ;;
    grok)
      printf '%s' "$PROMPT" >"$scratch/prompt.txt"
      args=(--prompt-file "$scratch/prompt.txt" --output-format streaming-json --sandbox read-only)
      [ -n "$session" ] && args+=(--resume "$session")
      (cd "$scratch" && timeout "$TIMEOUT_S" "${env_prefix[@]}" grok "${args[@]}") >"$scratch/raw.jsonl" 2>/dev/null </dev/null || code=$?
      ;;
  esac
  return "$code"
}

session_id_of() { # cli raw-file
  case "$1" in
    claude) first_match '"session_id":"[^"]+"' "$2" ;;
    codex) first_match '"thread_id":"[^"]+"' "$2" ;;
    grok) first_match '"sessionId":"[^"]+"' "$2" ;;
  esac
}

# finish CLI VERSION BEHAVIOUR RAW SCRATCH
finish() {
  local cli="$1" version="$2" behaviour="$3" raw="$4" scratch="$5"
  local name="$cli-$version-real-$behaviour.jsonl" final partial
  final="$OUT/$name"; partial="$final.partial"
  mkdir -p "$OUT" || { say "  FAILED  $name: cannot create $OUT"; return 1; }
  scrub "$scratch" <"$raw" >"$partial"
  if [ ! -s "$partial" ]; then
    rm -f "$partial"; record_result "$cli" "$version" "$behaviour" "$name" "failed" "the CLI printed nothing"
    say "  FAILED  $name: the CLI printed nothing (is it signed in, or the key set?)"; return 1
  fi
  local hit
  if hit="$(leaks "$partial")"; then
    rm -f "$partial"; record_result "$cli" "$version" "$behaviour" "$name" "leak" "credential-looking string '$hit'"
    say "  LEAK    $name: found '$hit' after scrubbing; the recording was deleted"; return 4
  fi
  if ! mv -f "$partial" "$final"; then
    rm -f "$partial"; record_result "$cli" "$version" "$behaviour" "$name" "failed" "cannot write $final"
    say "  FAILED  $name: cannot write $final"; return 1
  fi
  record_result "$cli" "$version" "$behaviour" "$name" "recorded"
  say "  wrote   $final"
}

status=0
for cli in $LIST; do
  if ! command -v "$cli" >/dev/null 2>&1; then
    if [ "$CLIS" = all ]; then
      say "$cli: not installed, skipped"
      record_result "$cli" "" "" "" "skipped" "not installed"
      continue
    fi
    die 3 "$cli is not installed (not on PATH). Install it, sign in or set its API key, then run this again. Nothing was written."
  fi
  version="$(version_of "$cli")"
  if [ -z "$version" ]; then
    die 3 "cannot read a version from '$cli --version'. Fixture names carry the CLI version, so this stops here."
  fi
  say "$cli $version"
  for behaviour in "${BEHAVIOUR_LIST[@]}"; do
    name="$cli-$version-real-$behaviour.jsonl"
    if [ "$cli" = codex ] && [ "$behaviour" = deltas ]; then
      say "  skip    $name: codex exec sends each message whole (deltas are an app-server feature)"
      record_result "$cli" "$version" "$behaviour" "$name" "skipped" "no partial text in codex exec"
      continue
    fi
    if [ "$YES" -ne 1 ]; then
      verb="would write"; [ -e "$OUT/$name" ] && verb="would replace"
      say "  $verb $OUT/$name"
      record_result "$cli" "$version" "$behaviour" "$name" "planned"
      continue
    fi
    scratch="$(mktemp -d)"
    trap 'rm -rf "$scratch"' EXIT
    code=0
    if [ "$behaviour" = resume ]; then
      run_cli "$cli" success "$scratch" || code=$?
      if [ "$code" -ne 0 ]; then
        record_result "$cli" "$version" "$behaviour" "$name" "failed" "the first turn exited $code"
        say "  FAILED  $name: the first turn exited $code"; status=4; rm -rf "$scratch"; continue
      fi
      sid="$(session_id_of "$cli" "$scratch/raw.jsonl")"
      if [ -z "$sid" ]; then
        record_result "$cli" "$version" "$behaviour" "$name" "failed" "no session id in the first turn"
        say "  FAILED  $name: no session id in the first turn"; status=4; rm -rf "$scratch"; continue
      fi
      run_cli "$cli" resume "$scratch" "$sid" || code=$?
    else
      run_cli "$cli" "$behaviour" "$scratch" || code=$?
    fi
    # a rejected key makes the CLI exit non-zero; that is the recording
    if [ "$code" -ne 0 ] && [ "$behaviour" != auth-expired ]; then
      record_result "$cli" "$version" "$behaviour" "$name" "failed" "exit $code"
      say "  FAILED  $name: the CLI exited $code"; status=4; rm -rf "$scratch"; continue
    fi
    finish "$cli" "$version" "$behaviour" "$scratch/raw.jsonl" "$scratch" || status=4
    rm -rf "$scratch"
  done
done

if [ "$YES" -ne 1 ]; then
  say "plan only: nothing was run or written. Add --yes to record."
fi
if [ "$JSON" -eq 1 ]; then
  ( IFS=,; printf '[%s]\n' "${RESULTS[*]:-}" )
fi
exit "$status"
