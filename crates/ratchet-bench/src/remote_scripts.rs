//! Shell scripts for the remote (bench instance) pipeline (port of `remote-scripts.ts`). One script per step; every script is
//! wrapped by [`wrap_step`] so it runs under bash even when the SSM document uses `sh`. The generated text is pinned byte for
//! byte against the TypeScript output (`tests/golden`).
//!
//! Results travel on stdout as sentinel lines `@@RATCHET@@ <kind> <json>`, because SSM returns at most ~24,000 characters of
//! output per command: the bench JSON is sent as the last 12,000 bytes of stdout, base64 encoded, and each bench run is its
//! own command so the budget is per run.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::deps::{DepsStrategy, FileHashes};
use crate::errors::{BenchError, BenchResult};
use crate::image_scripts::{
    ContainerNetwork, ContainerRunSpec, ContainerRuntime, HELPER_IN_CONTAINER, ImageRestore,
    assert_container_runtime, container_run, helper_mounts, ids_fragment, image_restore_fragment,
    launcher_conf_fragment,
};
pub use crate::shell::{BENCH_USER, HOLDOUT_USER, SENTINEL, assert_safe_ref, sq};
use crate::shell::{fill, json_str, reject_nul};

/// Root-installed launcher (`benchUserData`): the harness runs `$RATCHET_CANDIDATE_EXEC <cmd...>` to start a candidate
/// process as BENCH_USER, in the candidate's checkout, with a clean environment (no `RATCHET_TRACE`, no `AWS_*`).
/// stdin/stdout/stderr are the caller's: give the candidate pipes or `ignore`, not the harness's own stdout, which carries
/// the result.
pub const CANDIDATE_LAUNCHER: &str = "/usr/local/bin/ratchet-candidate";

/// Where one archive was staged (see `stage`) and the digest the instance must see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedArchive {
    /// `s3://<artifacts bucket>/bench-inputs/<opt>/<job>/<name>.tgz`
    pub uri: String,
    pub sha256: String,
    /// Full commit sha the archive was made from.
    pub commit: String,
    /// sha256 of the requested root-level files of the staged tree (lockfiles), computed on the controller before archiving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<FileHashes>,
    /// sha256 per file of the repo's container environment in the staged tree (see `image::environment_files_of`), when the
    /// job runs in a container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<FileHashes>,
}

impl StagedArchive {
    pub fn new(
        uri: impl Into<String>,
        sha256: impl Into<String>,
        commit: impl Into<String>,
    ) -> Self {
        StagedArchive {
            uri: uri.into(),
            sha256: sha256.into(),
            commit: commit.into(),
            files: None,
            env: None,
        }
    }
}

/// A dependency snapshot to restore before setup (see `deps`).
#[derive(Debug, Clone)]
pub struct DepsRestore {
    pub strategy: DepsStrategy,
    /// `s3://<artifacts bucket>/bench-deps/<key>/<sha256>.<ext>`
    pub uri: String,
    pub sha256: String,
    pub compression: crate::deps::Compression,
    /// `node --version` and the package manager's version on the machine that built it; a different image is refused.
    pub node: String,
    pub pm: String,
}

/// Per-job working directory on the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub work: String,
}

impl Layout {
    pub fn new(work: impl Into<String>) -> Self {
        Layout { work: work.into() }
    }
}

/// Where the restored store lives on the instance (read-only) and the directory the candidate user may write for the package
/// manager.
pub fn deps_paths(l: &Layout) -> (String, String) {
    (
        format!("{}/deps/store", l.work),
        format!("{}/home/.ratchet-deps", l.work),
    )
}

/// `user`: setup and guardrails run as BENCH_USER, the bench harness as HOLDOUT_USER, IMDS blocked for both (default,
/// fail-closed). `none`: single-user, for local tests only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    #[default]
    User,
    None,
}

impl Isolation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Isolation::User => "user",
            Isolation::None => "none",
        }
    }
}

const HEREDOC: &str = "__RATCHET_STEP_EOF__";

/// Make a bash script safe to hand to a shell that may not be bash: write it to a file and run it with bash.
pub fn wrap_step(body: &str) -> BenchResult<String> {
    if body.contains(HEREDOC) {
        return Err(BenchError::infra("script contains the step delimiter"));
    }
    Ok(format!(
        "f=$(mktemp)\ncat > \"$f\" <<'{HEREDOC}'\n{body}\n{HEREDOC}\nbash \"$f\"\nrc=$?\nrm -f \"$f\"\nexit $rc\n"
    ))
}

fn normalize_glob(g: &str) -> String {
    let mut s = g.replace('\\', "/");
    while let Some(rest) = s.strip_prefix("./") {
        s = rest.to_owned();
    }
    let s = s.trim_start_matches('/');
    s.trim_end_matches('/').to_owned()
}

/// `String(new RegExp(source).source)` for the patterns this module builds: V8 escapes a `/` outside a character class and
/// the line terminators.
fn v8_regexp_source(re: &str) -> String {
    let mut out = String::with_capacity(re.len() + 8);
    let mut escaped = false;
    let mut in_class = false;
    for c in re.chars() {
        if escaped {
            escaped = false;
            match c {
                '\n' => out.push('n'),
                '\r' => out.push('r'),
                '\u{2028}' => out.push_str("u2028"),
                '\u{2029}' => out.push_str("u2029"),
                _ => out.push(c),
            }
            continue;
        }
        match c {
            '\\' => {
                out.push(c);
                escaped = true;
            }
            '/' if !in_class => out.push_str("\\/"),
            '[' => {
                in_class = true;
                out.push(c);
            }
            ']' => {
                in_class = false;
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}

/// The body of `globToRegExp(glob).source` (anchors stripped), i.e. the regex the TypeScript compiled a glob to. The glob
/// dialect and its matcher live in `ratchet_core::protected`; this is the same translation, as text, for `grep -P`.
pub fn glob_regex_body(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut re = String::new();
    let mut in_brace = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '*' {
            if chars.get(i + 1) == Some(&'*') {
                i += 1;
                if chars.get(i + 1) == Some(&'/') {
                    i += 1;
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            } else {
                re.push_str("[^/]*");
            }
        } else if c == '?' {
            re.push_str("[^/]");
        } else if c == '[' {
            let close = chars
                .iter()
                .enumerate()
                .skip(i + 2)
                .find(|(_, ch)| **ch == ']')
                .map(|(idx, _)| idx);
            match close {
                None => re.push_str("\\["),
                Some(close) => {
                    let mut body: String = chars[i + 1..close].iter().collect();
                    if let Some(rest) = body.strip_prefix('!') {
                        body = format!("^{rest}");
                    }
                    re.push('[');
                    re.push_str(&body.replace('\\', "\\\\"));
                    re.push(']');
                    i = close;
                }
            }
        } else if c == '{' && !in_brace && chars[i..].contains(&'}') {
            in_brace = true;
            re.push_str("(?:");
        } else if c == '}' && in_brace {
            in_brace = false;
            re.push(')');
        } else if c == ',' && in_brace {
            re.push('|');
        } else {
            if matches!(
                c,
                '.' | '+' | '^' | '$' | '(' | ')' | '|' | '\\' | '{' | '}' | ']'
            ) {
                re.push('\\');
            }
            re.push(c);
        }
        i += 1;
    }
    // new RegExp(`^${re}$`).source, then `.slice(1, -1)`
    v8_regexp_source(&re)
}

/// PCRE (for `grep -zP`) equivalent of core's `matches_glob` for a list of globs: a path matches when some leading directory
/// prefix (or the whole path) matches one of the globs. The glob -> regex translation is core's own, so local and remote
/// runners agree on what is protected.
pub fn protected_pcre<S: AsRef<str>>(globs: &[S]) -> String {
    let bodies: Vec<String> = globs
        .iter()
        .map(|g| normalize_glob(g.as_ref()))
        .filter(|g| !g.is_empty())
        .map(|g| glob_regex_body(&g))
        .collect();
    if bodies.is_empty() {
        return "(?!)".into();
    }
    format!("(?s)^(?:{})(?:/.*)?\\z", bodies.join("|"))
}

fn prelude(l: &Layout) -> String {
    format!(
        "set -uo pipefail\nW={}\nCODE=\"$W/code\"; STAGE=\"$W/stage\"; G=\"$W/repo.git\"; H=\"$W/harness\"\nemit() {{ printf '%s %s %s\\n' '{SENTINEL}' \"$1\" \"$2\"; }}\nb64() {{ base64 | tr -d '\\n'; }}\n",
        sq(&l.work)
    )
}

/// Functions that implement the protected overlay (see `overlay` for the rule).
const OVERLAY_FNS: &str = r#"
parents_real() {
  local d acc="$CODE" part; local -a parts
  d=$(dirname -- "$1"); [ "$d" = . ] && return 0
  IFS=/ read -ra parts <<< "$d"
  for part in "${parts[@]}"; do acc="$acc/$part"; [ -L "$acc" ] && return 1; [ -d "$acc" ] || return 1; done
  return 0
}
ensure_parents() {
  local d acc="$CODE" part; local -a parts
  d=$(dirname -- "$1"); [ "$d" = . ] && return 0
  IFS=/ read -ra parts <<< "$d"
  for part in "${parts[@]}"; do
    acc="$acc/$part"
    if [ -L "$acc" ] || { [ -e "$acc" ] && [ ! -d "$acc" ]; }; then rm -f -- "${acc:?}"; fi
    [ -d "$acc" ] || mkdir -- "$acc"
  done
}
same() {
  if [ -L "$1" ]; then [ -L "$2" ] && [ "$(readlink -- "$1")" = "$(readlink -- "$2")" ]
  else [ -f "$2" ] && [ ! -L "$2" ] && cmp -s -- "$1" "$2"; fi
}
copied=0
apply() {
  local mode="$1" p
  : > "$W/changed.list"
  while IFS= read -r -d '' p; do
    if [ "$mode" = check ] && parents_real "$p" && same "$STAGE/$p" "$CODE/$p"; then continue; fi
    [ "$mode" = check ] && printf '%s\0' "$p" >> "$W/changed.list"
    ensure_parents "$p"
    rm -rf -- "${CODE:?}/${p:?}"
    cp -P -p -- "$STAGE/$p" "$CODE/$p" || return 1
    copied=$((copied+1))
  done < "$W/bench.match"
}
"#;

/// Wait-for-instance probe.
pub fn ping_script() -> &'static str {
    "echo ready\n"
}

/// Environment the candidate launcher reads (it runs as BENCH_USER through sudo, which keeps exactly these variables). Set by
/// root before any harness starts.
const CANDIDATE_ENV: &str = "export RATCHET_CANDIDATE_DIR=\"$CODE\" RATCHET_CANDIDATE_HOME=\"$W/home\" RATCHET_CANDIDATE_TMP=\"$W/tmp\" RATCHET_CANDIDATE_PATH=\"$PATH\"\n";

/// Checks that the instance really has the two unprivileged users, the IMDS block for both, the launcher, and that the
/// holdout directory is closed to the candidate's user and open to the harness's; refuses to continue otherwise (exit 12 to
/// 19). Then hands the code tree to the candidate user and the scratch dirs to the harness user.
fn isolation_check(isolation: Isolation, launcher: &str, container: bool) -> String {
    if isolation == Isolation::None {
        return String::new();
    }
    let launcher_probe = if container {
        // the harness starts candidate processes through the container launcher, which the image restore probes
        String::new()
    } else {
        "[ \"$(setpriv --reuid=\"$HU\" --regid=\"$HU\" --clear-groups -- \"$LAUNCHER\" id -un 2>/dev/null)\" = \"$BU\" ] || { echo \"the candidate launcher $LAUNCHER does not run commands as $BU for $HU\" >&2; exit 18; }\n".to_owned()
    };
    fill(
        r#"BU={{bu}}; HU={{hu}}; LAUNCHER={{launcher}}
id -u "$BU" >/dev/null 2>&1 || { echo "bench user $BU does not exist on this image" >&2; exit 12; }
id -u "$HU" >/dev/null 2>&1 || { echo "holdout user $HU does not exist on this image" >&2; exit 12; }
for u in "$BU" "$HU"; do
  iptables -C OUTPUT -m owner --uid-owner "$u" -d 169.254.169.254 -j REJECT >/dev/null 2>&1 || { echo "IMDS is not blocked for $u (iptables owner rule missing)" >&2; exit 13; }
done
if setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo "instance metadata is reachable as $BU" >&2; exit 14; fi
if setpriv --reuid="$HU" --regid="$HU" --clear-groups -- timeout 3 bash -c 'exec 3<>/dev/tcp/169.254.169.254/80' 2>/dev/null; then echo "instance metadata is reachable as $HU" >&2; exit 14; fi
mkdir -p "$W/home" "$W/tmp" "$W/hhome" "$W/htmp" "$W/out" "$W/holdout" || exit 15
chown -R "$BU:" "$CODE" "$W/home" "$W/tmp" || exit 16
chown -R "$HU:" "$W/hhome" "$W/htmp" "$W/out" "$W/holdout" || exit 16
chmod 700 "$W/holdout"
chmod 755 "$W"
{{candidate_env}}printf x > "$W/holdout/.probe" && chown "$HU:" "$W/holdout/.probe" && chmod 600 "$W/holdout/.probe" || exit 15
if setpriv --reuid="$BU" --regid="$BU" --clear-groups --no-new-privs -- cat "$W/holdout/.probe" >/dev/null 2>&1; then echo "the holdout directory is readable by $BU" >&2; exit 17; fi
setpriv --reuid="$HU" --regid="$HU" --clear-groups -- cat "$W/holdout/.probe" >/dev/null 2>&1 || { echo "the holdout directory is not readable by $HU" >&2; exit 19; }
rm -f "$W/holdout/.probe"
{{launcher_probe}}"#,
        &[
            ("bu", &sq(BENCH_USER)),
            ("hu", &sq(HOLDOUT_USER)),
            ("launcher", &sq(launcher)),
            ("candidate_env", CANDIDATE_ENV),
            ("launcher_probe", &launcher_probe),
        ],
    )
}

/// Restores the dependency snapshot as ROOT, after the isolation checks and before any step runs candidate code: download
/// (through the S3 gateway endpoint), sha256 check, same node and package-manager version as the builder, extract, refuse
/// symlinks, let the strategy wire its few writable paths, then hand the whole store to root and make it read-only. The
/// candidate user can read the store (setup installs from it) but not change it; the only things it owns are `$SCRATCH` (the
/// package manager's per-project bookkeeping, outside the store). Exit 25 to 29.
fn deps_restore_fragment(r: &DepsRestore, isolated: bool, container: bool) -> BenchResult<String> {
    static URI: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^s3://[a-z0-9][a-z0-9.-]*/bench-deps/[A-Za-z0-9._/-]+$")
            .expect("static regex")
    });
    if !URI.is_match(&r.uri) || r.uri.contains("..") {
        return Err(BenchError::infra(format!(
            "snapshot uri {} is not a plain s3:// uri under bench-deps/",
            json_str(&r.uri)
        )));
    }
    if !crate::deps::is_sha256_hex(&r.sha256) {
        return Err(BenchError::infra(
            "snapshot digest is not a sha256 hex string",
        ));
    }
    if !crate::deps::is_version(&r.node) || !crate::deps::is_version(&r.pm) {
        return Err(BenchError::infra(
            "snapshot tool versions are not plain version strings",
        ));
    }
    // In container mode the toolchain is the image's: the versions are asked from a throwaway container (offline,
    // unprivileged).
    let tool = |cmd: &str| -> String {
        if container {
            format!("ctr {}", sq(cmd))
        } else {
            cmd.to_owned()
        }
    };
    let extract = match r.compression {
        crate::deps::Compression::Zstd => {
            "command -v zstd >/dev/null 2>&1 || { echo \"zstd is missing on this image (rebuild the bench image)\" >&2; exit 28; }\nzstd -dc -- \"$D/in/snapshot\" | tar -x -C \"$STORE\" --no-same-owner --no-same-permissions || exit 29"
        }
        crate::deps::Compression::Gzip => {
            "tar -xzf \"$D/in/snapshot\" -C \"$STORE\" --no-same-owner --no-same-permissions || exit 29"
        }
    };
    let where_ = if container { "container" } else { "instance" };
    let tool_node = tool("node --version");
    let tool_pm = tool(r.strategy.version);
    Ok(fill(
        r#"# ---- dependency snapshot (root): read-only for the candidate user ----
D="$W/deps"; STORE="$D/store"; SCRATCH="$W/home/.ratchet-deps"
mkdir -p "$STORE" "$D/in" "$SCRATCH" || exit 25
aws s3 cp --only-show-errors {{uri_q}} "$D/in/snapshot" || { echo "cannot download the dependency snapshot {{uri}}" >&2; exit 25; }
[ "$(sha256sum -- "$D/in/snapshot" | cut -d' ' -f1)" = {{sha_q}} ] || { echo "sha256 mismatch for the dependency snapshot" >&2; exit 26; }
[ "$({{tool_node}} 2>/dev/null)" = {{node_q}} ] || { echo "the dependency snapshot was built with node {{node}}, this {{where}} has $({{tool_node}} 2>&1)" >&2; exit 27; }
[ "$({{tool_pm}} 2>/dev/null)" = {{pm_q}} ] || { echo "the dependency snapshot was built with {{strategy}} {{pm}}, this {{where}} has $({{tool_pm}} 2>&1)" >&2; exit 27; }
{{extract}}
rm -f "$D/in/snapshot"
if [ -n "$(find "$STORE" -type l -print -quit)" ]; then echo "the dependency snapshot contains symlinks" >&2; exit 29; fi
{{restore}}
{{chowns}}chmod -R go-w,a+rX "$D" || exit 16
emit deps "{\"restored\":true,\"bytes\":$(du -sb "$STORE" | cut -f1)}"
"#,
        &[
            ("uri_q", &sq(&r.uri)),
            ("uri", &r.uri),
            ("sha_q", &sq(&r.sha256)),
            ("tool_node", &tool_node),
            ("tool_pm", &tool_pm),
            ("node_q", &sq(&r.node)),
            ("node", &r.node),
            ("pm_q", &sq(&r.pm)),
            ("pm", &r.pm),
            ("strategy", r.strategy.id.as_str()),
            ("where", where_),
            ("extract", extract),
            ("restore", r.strategy.restore),
            (
                "chowns",
                if isolated {
                    "chown -R \"$BU:\" \"$SCRATCH\" || exit 16\nchown -hR root:root \"$D\" || exit 16\n"
                } else {
                    ""
                },
            ),
        ],
    ))
}

/// Input of [`checkout_script`].
#[derive(Debug, Clone)]
pub struct CheckoutScriptInput {
    pub layout: Layout,
    pub code: StagedArchive,
    pub bench: StagedArchive,
    pub globs: Vec<String>,
    /// Always-protected root paths (lockfiles of the dependency strategy), on top of `globs`.
    pub pinned: Vec<String>,
    pub prune: bool,
    pub isolation: Isolation,
    /// Restore this dependency snapshot before setup.
    pub deps: Option<DepsRestore>,
    /// Load the repo's container image (offline) and probe it before setup; setup, guardrails and the evaluator then run in
    /// it.
    pub image: Option<ImageRestore>,
    /// Test seam; default [`CANDIDATE_LAUNCHER`].
    pub candidate_launcher: Option<String>,
}

impl CheckoutScriptInput {
    pub fn new(
        layout: Layout,
        code: StagedArchive,
        bench: StagedArchive,
        globs: Vec<String>,
    ) -> Self {
        CheckoutScriptInput {
            layout,
            code,
            bench,
            globs,
            pinned: Vec::new(),
            prune: true,
            isolation: Isolation::User,
            deps: None,
            image: None,
            candidate_launcher: None,
        }
    }
}

/// Fetches the pre-staged candidate and bench.ref archives from the artifacts bucket (through the S3 gateway endpoint; the
/// bench subnet has no other way out), verifies their digests, applies the protected overlay, and - with
/// `isolation: user` - verifies the two unprivileged users, the IMDS block, the holdout directory permissions and the
/// candidate launcher, and hands the code tree to the candidate user. No source-control access and no credentials in the
/// script, and the sealed bucket is not touched: the holdout is fetched later, by the bench step that needs it.
///
/// With `deps` the dependency snapshot is restored (read-only) last, still as root and before any candidate code ran.
/// `pinned` are extra protected paths (the lockfiles): they come from bench.ref exactly like `globs`, but do not count as
/// "the user protected something" in the `matched` figure.
pub fn checkout_script(o: &CheckoutScriptInput) -> BenchResult<String> {
    static URI: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^s3://[a-z0-9][a-z0-9.-]*/[A-Za-z0-9._/-]+$").expect("static regex")
    });
    for a in [&o.code, &o.bench] {
        assert_safe_ref(&a.commit, "staged commit")?;
        if !URI.is_match(&a.uri) || a.uri.contains("..") {
            return Err(BenchError::infra(format!(
                "staged archive uri {} is not a plain s3:// uri",
                json_str(&a.uri)
            )));
        }
        if !crate::deps::is_sha256_hex(&a.sha256) {
            return Err(BenchError::infra(
                "staged archive digest is not a sha256 hex string",
            ));
        }
    }
    let launcher = o
        .candidate_launcher
        .as_deref()
        .unwrap_or(CANDIDATE_LAUNCHER);
    reject_nul(
        [o.layout.work.as_str(), launcher]
            .into_iter()
            .chain(o.globs.iter().map(String::as_str))
            .chain(o.pinned.iter().map(String::as_str)),
    )?;
    let all_globs: Vec<&str> = o
        .globs
        .iter()
        .chain(o.pinned.iter())
        .map(String::as_str)
        .collect();
    let isolated = o.isolation == Isolation::User;
    let image_fragment = match &o.image {
        Some(i) => image_restore_fragment(i, &o.layout.work, isolated)?,
        None => String::new(),
    };
    let deps_fragment = match &o.deps {
        Some(d) => deps_restore_fragment(d, isolated, o.image.is_some())?,
        None => String::new(),
    };
    let body = fill(
        r#"
RE={{re}}; RE_USER={{re_user}}
rm -rf -- "${W:?}"; mkdir -p "$CODE" "$STAGE" "$W/logs" "$W/in" || exit 10
fetch() {
  aws s3 cp --only-show-errors "$1" "$3" || { echo "cannot download $1" >&2; return 1; }
  [ "$(sha256sum -- "$3" | cut -d' ' -f1)" = "$2" ] || { echo "sha256 mismatch for $1" >&2; return 2; }
}
fetch {{code_uri}} {{code_sha}} "$W/in/code.tgz" || exit 20
fetch {{bench_uri}} {{bench_sha}} "$W/in/bench.tgz" || exit 21
tar -xzf "$W/in/code.tgz" -C "$CODE" --no-same-owner || exit 22
tar -xzf "$W/in/bench.tgz" -C "$STAGE" --no-same-owner || exit 23
list() { (cd -- "$1" && find . -mindepth 1 \( -type f -o -type l \) -printf '%P\0'); }
list "$STAGE" | grep -zP "$RE" > "$W/bench.match" || true
matched=$(list "$STAGE" | grep -zcP "$RE_USER" || true)
list "$CODE" | grep -zP "$RE" > "$W/code.match" || true
removed=0
if [ {{prune}} = 1 ]; then
  CODE_REAL=$(cd "$CODE" && pwd -P)
  while IFS= read -r -d '' p; do
    d=$(dirname -- "$p")
    if [ "$d" = . ] || [ "$(realpath -m -- "$CODE/$d")" = "$CODE_REAL/$d" ]; then
      rm -rf -- "${CODE:?}/${p:?}"
      grep -zqxF -- "$p" "$W/bench.match" || removed=$((removed+1))
    fi
  done < "$W/code.match"
fi
apply init || exit 24
{{isolation_check}}{{image}}{{deps}}emit checkout "{\"code\":\"{{code_commit}}\",\"bench\":\"{{bench_commit}}\",\"removed\":$removed,\"copied\":$copied,\"matched\":$matched,\"isolation\":\"{{isolation}}\",\"container\":{{container}}}"
"#,
        &[
            ("re", &sq(&protected_pcre(&all_globs))),
            ("re_user", &sq(&protected_pcre(&o.globs))),
            ("code_uri", &sq(&o.code.uri)),
            ("code_sha", &sq(&o.code.sha256)),
            ("bench_uri", &sq(&o.bench.uri)),
            ("bench_sha", &sq(&o.bench.sha256)),
            ("prune", if o.prune { "1" } else { "0" }),
            (
                "isolation_check",
                &isolation_check(o.isolation, launcher, o.image.is_some()),
            ),
            ("image", &image_fragment),
            ("deps", &deps_fragment),
            ("code_commit", &o.code.commit),
            ("bench_commit", &o.bench.commit),
            ("isolation", o.isolation.as_str()),
            (
                "container",
                if o.image.is_some() { "true" } else { "false" },
            ),
        ],
    );
    Ok(format!("{}{OVERLAY_FNS}{body}", prelude(&o.layout)))
}

/// Which step a script runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepKind {
    Setup,
    Guardrail,
    Bench,
}

#[derive(Debug, Clone)]
pub struct StepScriptInput {
    pub layout: Layout,
    pub kind: StepKind,
    /// Used for log file names; sanitized.
    pub name: String,
    pub command: String,
    pub timeout_ms: u64,
    pub env: IndexMap<String, String>,
    /// Default [`Isolation::User`].
    pub isolation: Isolation,
    /// Confirm bench runs only: the sealed holdout to fetch for THIS run (`s3://...`, or a path / `file://` in tests). The
    /// step fetches it as root, hands it to the harness user alone (mode 0600), exports `RATCHET_TRACE` to the harness and
    /// wipes it when the run is over. Never set for setup, guardrails, baseline or bench-phase runs: those scripts do not
    /// contain the sealed location at all.
    pub holdout: Option<String>,
    /// Run the step INSIDE the repo's container image (`environment.image`), loaded at checkout: `docker run --network
    /// none`, the same numeric uids as the host path, all capabilities dropped (the harness keeps SETUID/SETGID for its
    /// launcher), read-only root, tmpfs /tmp, and only the checkout, the homes, the out dir (and, for a confirm harness, the
    /// holdout, read-only) mounted.
    pub container: Option<ContainerRuntime>,
    /// The dependency snapshot was restored at `$W/deps`: mount it read-only into setup / guardrail containers.
    pub deps_mounted: bool,
    /// Test seam; default [`CANDIDATE_LAUNCHER`].
    pub candidate_launcher: Option<String>,
}

impl StepScriptInput {
    pub fn new(
        layout: Layout,
        kind: StepKind,
        name: impl Into<String>,
        command: impl Into<String>,
        timeout_ms: u64,
    ) -> Self {
        StepScriptInput {
            layout,
            kind,
            name: name.into(),
            command: command.into(),
            timeout_ms,
            env: IndexMap::new(),
            isolation: Isolation::User,
            holdout: None,
            container: None,
            deps_mounted: false,
            candidate_launcher: None,
        }
    }
}

/// Exit codes of the holdout fetch inside a bench step (the executor turns them into an infrastructure error).
pub mod holdout_exit {
    pub const PREPARE: u8 = 40;
    pub const DOWNLOAD: u8 = 41;
    pub const PERMISSIONS: u8 = 42;
}

/// The three codes as a slice.
pub const HOLDOUT_FETCH_EXIT: [i32; 3] = [
    holdout_exit::PREPARE as i32,
    holdout_exit::DOWNLOAD as i32,
    holdout_exit::PERMISSIONS as i32,
];

fn is_env_name(k: &str) -> bool {
    let mut chars = k.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `name.replace(/[^A-Za-z0-9._-]/g, '_').slice(0, 60)` (JavaScript counts and replaces UTF-16 code units).
fn log_file_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('_');
            }
        }
    }
    out.chars().take(60).collect()
}

/// Setup, guardrail and bench steps. Before running the command the script verifies that the protected files still equal
/// bench.ref's; if not it restores them, reports `tampered`, and exits 87 without running the command.
///
/// With `isolation: user` there are two unprivileged principals:
///
///  - setup and guardrails run as BENCH_USER (candidate code, `--no-new-privs`);
///  - a BENCH step (the evaluator harness) runs as HOLDOUT_USER, from a root-owned read-only SNAPSHOT of the built tree taken
///    right before the first run, so the candidate cannot edit what the harness executes. The harness starts candidate
///    processes through `$RATCHET_CANDIDATE_EXEC`, which runs them as BENCH_USER. The holdout (confirm only) is fetched
///    here, after setup and guardrails have finished, written mode 0600 for HOLDOUT_USER and wiped after the run (also on
///    any exit), so no candidate process ever has a window in which it could open it.
///
/// The harness can run unprivileged-but-not-`no-new-privs`: the launcher is a sudo rule, which setuid needs. Everything the
/// harness executes should therefore come from `protected` paths.
pub fn step_script(o: &StepScriptInput) -> BenchResult<String> {
    let timeout_s = o.timeout_ms.div_ceil(1000).max(1);
    let file = log_file_name(&o.name);
    for k in o.env.keys() {
        if !is_env_name(k) {
            return Err(BenchError::infra(format!(
                "invalid environment variable name {k}"
            )));
        }
    }
    if o.holdout.is_some() && o.kind != StepKind::Bench {
        return Err(BenchError::infra(
            "the holdout is only ever given to bench steps",
        ));
    }
    if o.env.contains_key("RATCHET_TRACE") {
        return Err(BenchError::infra(
            "RATCHET_TRACE is set by the step script itself, from `holdout`",
        ));
    }
    let launcher = o
        .candidate_launcher
        .as_deref()
        .unwrap_or(CANDIDATE_LAUNCHER);
    reject_nul(
        [
            o.layout.work.as_str(),
            o.command.as_str(),
            launcher,
            o.holdout.as_deref().unwrap_or(""),
        ]
        .into_iter()
        .chain(o.env.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()])),
    )?;
    let exports = o
        .env
        .iter()
        .map(|(k, v)| format!("export {k}={}", sq(v)))
        .collect::<Vec<_>>()
        .join("\n");
    let isolated = o.isolation == Isolation::User;
    let harness = isolated && o.kind == StepKind::Bench;
    let result = if o.kind == StepKind::Bench {
        // The log of a holdout run carries no free text from the run: not the harness's stderr (which can relay whatever
        // the candidate's processes wrote to it).
        let tail = if o.holdout.is_some() {
            "echo \"--- stderr withheld: holdout run ---\""
        } else {
            "echo \"--- stderr tail ---\"; tail -c 3000 \"$OUT.stderr\""
        };
        format!(
            "emit run \"{{\\\"exit\\\":$rc,\\\"ms\\\":$ms,\\\"timedOut\\\":$to,\\\"stdout_b64\\\":\\\"$(tail -c 12000 \"$OUT.stdout\" | b64)\\\"}}\"\n{tail}"
        )
    } else {
        "emit step \"{\\\"exit\\\":$rc,\\\"ms\\\":$ms,\\\"timedOut\\\":$to}\"\necho \"--- output tail ---\"; { tail -c 4000 \"$OUT.stdout\"; tail -c 2000 \"$OUT.stderr\"; }".to_owned()
    };

    // Setup and guardrails: candidate code as BENCH_USER without new privileges. The bench harness: HOLDOUT_USER, with its
    // own HOME/TMPDIR/output dir. Both lose the AWS_* variables even though a stock SSM environment has none.
    let scrub =
        "env -u AWS_ACCESS_KEY_ID -u AWS_SECRET_ACCESS_KEY -u AWS_SESSION_TOKEN -u AWS_PROFILE";
    if let Some(c) = &o.container {
        assert_container_runtime(c, &o.layout.work)?;
    }
    let mut pass_env: Vec<String> = o.env.keys().cloned().collect();
    if o.holdout.is_some() {
        pass_env.push("RATCHET_TRACE".into());
    }
    let container_cmd = |container: &ContainerRuntime| -> String {
        if harness {
            let mut mounts: Vec<String> = vec![
                "$H:$H:ro".into(),
                "$CODE:$CODE".into(),
                "$W/home:$W/home".into(),
                "$W/tmp:$W/tmp".into(),
                "$W/hhome:$W/hhome".into(),
                "$W/htmp:$W/htmp".into(),
                "$W/out:$W/out".into(),
            ];
            if o.holdout.is_some() {
                mounts.push("$W/holdout:$W/holdout:ro".into());
            }
            mounts.extend(helper_mounts(container));
            container_run(&ContainerRunSpec {
                rt: container.clone(),
                name: "\"$CN\"".into(),
                user: "\"$HUID:$HGID\"".into(),
                // only for the launcher; the harness itself needs nothing
                cap_add: vec!["SETUID".into(), "SETGID".into()],
                // the launcher is setuid root; it sets no_new_privs on every candidate process
                no_new_privileges: false,
                network: ContainerNetwork::None,
                workdir: "\"$H\"".into(),
                mounts,
                pass_env: pass_env.clone(),
                env: vec![
                    "HOME=$W/hhome".into(),
                    "TMPDIR=$W/htmp".into(),
                    "USER=$HU".into(),
                    "LOGNAME=$HU".into(),
                    "RATCHET_OUT_DIR=$W/out".into(),
                    format!("RATCHET_CANDIDATE_EXEC={HELPER_IN_CONTAINER}"),
                    "RATCHET_CANDIDATE_DIR=$CODE".into(),
                    "RATCHET_CANDIDATE_HOME=$W/home".into(),
                    "RATCHET_CANDIDATE_TMP=$W/tmp".into(),
                ],
                extra: vec![],
                command: sq(&o.command),
            })
        } else {
            let mut mounts: Vec<String> = vec![
                "$CODE:$CODE".into(),
                "$W/home:$W/home".into(),
                "$W/tmp:$W/tmp".into(),
            ];
            if o.deps_mounted && o.kind != StepKind::Bench {
                mounts.push("$W/deps:$W/deps:ro".into());
            }
            if o.holdout.is_some() {
                mounts.push("$W/holdout:$W/holdout:ro".into());
            }
            let mut env = vec!["HOME=$W/home".to_owned(), "TMPDIR=$W/tmp".to_owned()];
            if isolated {
                env.push("USER=$BU".into());
                env.push("LOGNAME=$BU".into());
            }
            container_run(&ContainerRunSpec {
                rt: container.clone(),
                name: "\"$CN\"".into(),
                user: "\"$BUID:$BGID\"".into(),
                cap_add: vec![],
                no_new_privileges: true,
                network: ContainerNetwork::None,
                workdir: "\"$CODE\"".into(),
                mounts,
                pass_env: pass_env.clone(),
                env,
                extra: vec![],
                command: sq(&o.command),
            })
        }
    };
    let run = if let Some(c) = &o.container {
        container_cmd(c)
    } else if harness {
        format!(
            "setpriv --reuid=\"$HU\" --regid=\"$HU\" --clear-groups -- {scrub} HOME=\"$W/hhome\" TMPDIR=\"$W/htmp\" USER=\"$HU\" LOGNAME=\"$HU\" RATCHET_OUT_DIR=\"$W/out\" RATCHET_CANDIDATE_EXEC={} bash -c {}",
            sq(launcher),
            sq(&o.command)
        )
    } else if isolated {
        format!(
            "setpriv --reuid=\"$BU\" --regid=\"$BU\" --clear-groups --no-new-privs -- {scrub} HOME=\"$W/home\" TMPDIR=\"$W/tmp\" USER=\"$BU\" LOGNAME=\"$BU\" bash -c {}",
            sq(&o.command)
        )
    } else {
        format!("bash -c {}", sq(&o.command))
    };

    let fetch_holdout = match &o.holdout {
        Some(uri) => {
            let path = local_trace_path(&o.layout, uri);
            let perms = if isolated {
                format!(
                    "chown \"$HU:\" \"$TRACE\" || exit {p}\nchmod 600 \"$TRACE\" || exit {p}",
                    p = holdout_exit::PERMISSIONS
                )
            } else {
                format!("chmod 600 \"$TRACE\" || exit {}", holdout_exit::PERMISSIONS)
            };
            fill(
                r#"
# ---- holdout: fetched only now (after setup and guardrails), by root, for this run alone ----
HD="$W/holdout"; TRACE={{trace}}; URI={{uri}}
wipe() { shred -u -- "$TRACE" 2>/dev/null || rm -f -- "$TRACE"; rm -rf -- "${HD:?}"/* 2>/dev/null || true; }
trap wipe EXIT
mkdir -p "$HD" || exit {{prepare}}
case "$URI" in
  s3://*) aws s3 cp --only-show-errors "$URI" "$TRACE" || exit {{download}} ;;
  file://*) cp -- "${URI#file://}" "$TRACE" || exit {{download}} ;;
  *) cp -- "$URI" "$TRACE" || exit {{download}} ;;
esac
{{perms}}
emit holdout "{\"bytes\":$(wc -c < "$TRACE" | tr -d ' ')}"
export RATCHET_TRACE="$TRACE"
"#,
                &[
                    ("trace", &sq(&path)),
                    ("uri", &sq(uri)),
                    ("prepare", &holdout_exit::PREPARE.to_string()),
                    ("download", &holdout_exit::DOWNLOAD.to_string()),
                    ("perms", &perms),
                ],
            )
        }
        None => String::new(),
    };

    let snapshot = if harness {
        r#"
# ---- read-only snapshot of the built tree: what the harness (HOLDOUT_USER) executes ----
snapshot() {
  rm -rf -- "$W/harness.tmp"
  cp -a --reflink=auto -- "$CODE" "$W/harness.tmp" || return 1
  find "$W/harness.tmp" -type f -perm /6000 -exec chmod ug-s {} + || return 1
  chown -hR root:root "$W/harness.tmp" || return 1
  chmod -R go-w,a+rX "$W/harness.tmp" || return 1
  mv -- "$W/harness.tmp" "$H"
}
[ -d "$H" ] || snapshot || { echo "cannot snapshot the built tree for the evaluator" >&2; exit 31; }
"#
    } else {
        ""
    };

    let users = if isolated {
        format!(
            "BU={}; HU={}\nkill_users() {{ pkill -KILL -u \"$BU\" 2>/dev/null || true; pkill -KILL -u \"$HU\" 2>/dev/null || true; }}\nkill_users\n{CANDIDATE_ENV}",
            sq(BENCH_USER),
            sq(HOLDOUT_USER)
        )
    } else {
        String::new()
    };
    let container_setup = if o.container.is_some() {
        format!(
            "{}CN=\"ratchet-$(basename -- \"$W\")-{}\"\ndocker rm -f \"$CN\" >/dev/null 2>&1 || true\n",
            ids_fragment(isolated),
            file.replace('_', "-")
        )
    } else {
        String::new()
    };
    let conf = if o.container.is_some() && harness {
        launcher_conf_fragment()
    } else {
        String::new()
    };

    let body = fill(
        r#"
{{users}}{{container_setup}}
apply check
if [ -s "$W/changed.list" ]; then
  emit tampered "{\"paths_b64\":\"$(tr '\0' '\n' < "$W/changed.list" | head -c 4000 | b64)\"}"
  exit 87
fi
{{snapshot}}{{fetch_holdout}}{{conf}}cd {{cwd}} || exit 30
{{exports}}
OUT="$W/logs/{{file}}"
s=$(date +%s%N)
timeout -k 10 {{timeout_s}} {{run}} </dev/null >"$OUT.stdout" 2>"$OUT.stderr"; rc=$?
e=$(date +%s%N); ms=$(( (e - s) / 1000000 ))
{{container_rm}}
{{kill}}
{{wipe}}
to=false
if [ $rc -eq 124 ] || { [ $rc -eq 137 ] && [ $ms -ge $(( {{timeout_s}} * 1000 )) ]; }; then to=true; fi
{{result}}
exit 0
"#,
        &[
            ("users", &users),
            ("container_setup", &container_setup),
            ("snapshot", snapshot),
            ("fetch_holdout", &fetch_holdout),
            ("conf", &conf),
            ("cwd", if harness { "\"$H\"" } else { "\"$CODE\"" }),
            ("exports", &exports),
            ("file", &file),
            ("timeout_s", &timeout_s.to_string()),
            ("run", &run),
            (
                "container_rm",
                if o.container.is_some() {
                    "docker rm -f \"$CN\" >/dev/null 2>&1 || true"
                } else {
                    ""
                },
            ),
            ("kill", if isolated { "kill_users" } else { "" }),
            ("wipe", if o.holdout.is_some() { "wipe" } else { "" }),
            ("result", &result),
        ],
    );
    Ok(format!("{}{OVERLAY_FNS}{body}", prelude(&o.layout)))
}

/// One `@@RATCHET@@ <kind> <json>` line of a script's stdout.
#[derive(Debug, Clone, PartialEq)]
pub struct Sentinel {
    pub kind: String,
    pub data: Map<String, Value>,
}

pub fn parse_sentinels(stdout: &str) -> Vec<Sentinel> {
    let prefix = format!("{SENTINEL} ");
    let mut out = Vec::new();
    for line in stdout.split('\n') {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let Some(sp) = rest.find(' ') else { continue };
        // JavaScript's `typeof [] === "object"` let an array through with no usable fields: same here (empty data).
        let data = match serde_json::from_str::<Value>(&rest[sp + 1..]) {
            Ok(Value::Object(data)) => data,
            Ok(Value::Array(_)) => Map::new(),
            _ => continue, // a truncated line, or not an object
        };
        out.push(Sentinel {
            kind: rest[..sp].to_owned(),
            data,
        });
    }
    out
}

/// stdout without sentinel lines (they are structured data, not log text).
pub fn strip_sentinels(stdout: &str) -> String {
    let prefix = format!("{SENTINEL} ");
    stdout
        .split('\n')
        .filter(|l| !l.starts_with(&prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where the holdout lives on the instance (inside the 0700 holdout directory) while a bench run uses it.
pub fn local_trace_path(layout: &Layout, uri: &str) -> String {
    let ext: String = crate::posix::extname(uri.split('?').next().unwrap_or(""))
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
        .collect();
    format!("{}/holdout/trace{ext}", layout.work)
}
