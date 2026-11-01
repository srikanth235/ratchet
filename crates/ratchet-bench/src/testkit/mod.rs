//! Test doubles for the bench runners (port of `testkit/`): a tiny git repo with a bench script, a local `GitHost`, clocks,
//! artifact stores, a "bench instance" that is a local bash (`LocalFakeCompute`) and a `docker` shim for it.
//!
//! The fixture scripts are bash, not node: the Rust crate does not need a JavaScript runtime to test the bench.

mod compute;
mod docker;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::RatchetConfig;
use ratchet_core::ports::{
    ArtifactStore, CancellationToken, CandidateCommit, Clock, FileDiff, GitHost, PortError,
    PortResult, PullRequestInput, PullRequestRef,
};

pub use compute::LocalFakeCompute;
pub use docker::docker_shim;

/// A clock whose sleeps return immediately and are recorded.
#[derive(Default)]
pub struct InstantClock {
    pub slept: Mutex<Vec<u64>>,
}

impl InstantClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn slept(&self) -> Vec<u64> {
        self.slept.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl Clock for InstantClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(PortError::Cancelled);
        }
        if let Ok(mut s) = self.slept.lock() {
            s.push(ms);
        }
        Ok(())
    }
}

/// The real clock (core's).
pub fn real_clock() -> Arc<dyn Clock> {
    Arc::new(ratchet_core::ports::mock::SystemClock)
}

pub fn instant_clock() -> Arc<dyn Clock> {
    InstantClock::new()
}

/// A temporary directory.
pub fn tmp(prefix: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("temp dir")
}

/// `git <args>` in `cwd`; stdout trimmed. Panics on failure (test helper).
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// `GitHost` that materializes any ref with `git archive` (no .git in the result).
pub struct LocalGitHost {
    pub repo: PathBuf,
    checkouts: Mutex<Vec<(String, PathBuf)>>,
}

impl LocalGitHost {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        LocalGitHost {
            repo: repo.into(),
            checkouts: Mutex::new(Vec::new()),
        }
    }

    /// `(ref, dir)` of every checkout, in order.
    pub fn checkouts(&self) -> Vec<(String, PathBuf)> {
        self.checkouts.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl GitHost for LocalGitHost {
    async fn create_branch(&self, from_ref: &str, branch: &str) -> PortResult<()> {
        git(&self.repo, &["branch", branch, from_ref]);
        Ok(())
    }

    async fn list_candidates(
        &self,
        _branch: &str,
        _since: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        Ok(Vec::new())
    }

    async fn changed_files(&self, base_ref: &str, sha: &str) -> PortResult<Vec<String>> {
        Ok(git(&self.repo, &["diff", "--name-only", base_ref, sha])
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect())
    }

    async fn diff(&self, _base_ref: &str, _sha: &str) -> PortResult<Vec<FileDiff>> {
        Ok(Vec::new())
    }

    async fn checkout(&self, sha: &str, dir: &Path) -> PortResult<()> {
        if let Ok(mut c) = self.checkouts.lock() {
            c.push((sha.to_owned(), dir.to_path_buf()));
        }
        std::fs::create_dir_all(dir).map_err(|e| PortError::failed(e.to_string()))?;
        // Two steps, not `git archive | tar -x`: a pipe reports tar's status, and BSD tar (macOS) accepts the empty
        // archive that a failed `git archive` leaves behind, so an unknown sha would "check out" an empty tree.
        let archive = tokio::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["archive", "--format=tar", sha])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .await
            .map_err(|e| PortError::failed(e.to_string()))?;
        if !archive.status.success() {
            return Err(PortError::failed(
                String::from_utf8_lossy(&archive.stderr).trim().to_owned(),
            ));
        }
        let mut tar = tokio::process::Command::new("tar")
            .arg("-x")
            .arg("-C")
            .arg(dir)
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| PortError::failed(e.to_string()))?;
        if let Some(mut stdin) = tar.stdin.take() {
            tokio::io::AsyncWriteExt::write_all(&mut stdin, &archive.stdout)
                .await
                .map_err(|e| PortError::failed(e.to_string()))?;
        }
        let out = tar
            .wait_with_output()
            .await
            .map_err(|e| PortError::failed(e.to_string()))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(PortError::failed(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ))
        }
    }

    async fn open_pull_request(&self, _input: &PullRequestInput) -> PortResult<PullRequestRef> {
        Ok(PullRequestRef {
            url: "https://example.invalid/pr/1".into(),
        })
    }

    async fn resolve_ref(&self, git_ref: &str) -> PortResult<String> {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", &format!("{git_ref}^{{commit}}")])
            .current_dir(&self.repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .map_err(|e| PortError::failed(e.to_string()))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            Err(PortError::failed(
                String::from_utf8_lossy(&out.stderr).trim().to_owned(),
            ))
        }
    }

    fn remote_url(&self) -> String {
        self.repo.to_string_lossy().into_owned()
    }
}

/// The bench of the fixture repo: reads `src/*.txt`, prints a JSON object. Environment knobs: `MARKER` (append the pid, count
/// the runs), `JITTER` (+0.1 per run), `RATCHET_TRACE`, `RATCHET_RUN`, `RATCHET_PHASE`; `src/hang.txt` makes it sleep.
pub const BENCH_SH: &str = r#"#!/bin/bash
n=1
if [ -n "${MARKER:-}" ]; then
  echo $$ >> "$MARKER"
  n=$(wc -l < "$MARKER" | tr -d ' ')
fi
p95=$(awk -v p="$(cat src/p95.txt)" -v n="$n" -v j="${JITTER:-}" 'BEGIN { if (j != "") p = p + (n - 1) * 0.1; printf "%.10g", p }')
trace_set=0; trace_bytes=0
if [ -n "${RATCHET_TRACE:-}" ]; then trace_set=1; trace_bytes=$(wc -c < "$RATCHET_TRACE" | tr -d ' '); fi
confirm=0; [ "${RATCHET_PHASE:-}" = confirm ] && confirm=1
if [ -e src/hang.txt ]; then sleep 60; fi
echo "some log line"
printf '{"http_req_duration":{"p95":%s,"p50":1},"error_rate":%s,"peak_rss_mb":%s,"trace_set":%s,"trace_bytes":%s,"run":%s,"phase_is_confirm":%s,"label":"text","arr":[1,2]}\n' \
  "$p95" "$(cat src/err.txt)" "$(cat src/rss.txt)" "$trace_set" "$trace_bytes" "${RATCHET_RUN:-0}" "$confirm"
"#;

pub const CHECK_SH: &str = r#"#!/bin/bash
if grep -q BROKEN src/p95.txt; then echo "check failed: broken" >&2; exit 1; fi
"#;

/// The files of the base commit (and of the trusted `ratchet/bench` branch).
pub fn base_files() -> Vec<(String, String)> {
    [
        ("bench/bench.sh", BENCH_SH),
        ("bench/data/trace-notes.txt", "trusted data\n"),
        ("tests/check.sh", CHECK_SH),
        ("src/p95.txt", "100"),
        ("src/rss.txt", "100"),
        ("src/err.txt", "0"),
        ("ratchet.yaml", "# placeholder\n"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
    .collect()
}

/// A change a candidate commit makes to the base tree.
#[derive(Debug, Clone)]
pub enum Change {
    Write(String),
    Delete,
    Symlink(String),
}

impl Change {
    pub fn write(s: &str) -> Self {
        Change::Write(s.to_owned())
    }

    pub fn symlink(target: &str) -> Self {
        Change::Symlink(target.to_owned())
    }
}

pub struct FixtureRepo {
    pub dir: tempfile::TempDir,
    pub repo: PathBuf,
    pub host: Arc<LocalGitHost>,
    /// Commit sha of the base on branch `main` and of the trusted bench on branch `ratchet/bench`.
    pub base: String,
    n: Mutex<u32>,
}

impl FixtureRepo {
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Create a commit on top of base with these file changes; returns its sha.
    pub fn candidate(&self, changes: &[(&str, Change)]) -> String {
        let name = {
            let mut n = self.n.lock().unwrap_or_else(|e| e.into_inner());
            *n += 1;
            format!("cand-{n}")
        };
        let repo = &self.repo;
        git(repo, &["checkout", "-q", "-B", &name, &self.base]);
        for (f, c) in changes {
            let p = repo.join(f);
            let _ = std::fs::remove_dir_all(&p);
            let _ = std::fs::remove_file(&p);
            if let Change::Delete = c {
                continue;
            }
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            match c {
                Change::Write(body) => std::fs::write(&p, body).expect("write"),
                Change::Symlink(target) => std::os::unix::fs::symlink(target, &p).expect("symlink"),
                Change::Delete => {}
            }
        }
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "--allow-empty", "-m", &name]);
        let sha = git(repo, &["rev-parse", "HEAD"]);
        git(repo, &["checkout", "-q", "main"]);
        sha
    }
}

/// `extra` files are part of the base commit (and of the trusted `ratchet/bench` branch), e.g. a lockfile.
pub fn make_fixture_repo(extra: &[(&str, &str)]) -> FixtureRepo {
    let dir = tmp("ratchet-bench-test-");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).expect("mkdir repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    let mut files: BTreeMap<String, String> = base_files().into_iter().collect();
    for (f, c) in extra {
        files.insert((*f).to_owned(), (*c).to_owned());
    }
    for (f, c) in &files {
        let p = repo.join(f);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(&p, c).expect("write");
        if f.ends_with(".sh") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["branch", "ratchet/bench", &base]);
    FixtureRepo {
        host: Arc::new(LocalGitHost::new(&repo)),
        dir,
        repo,
        base,
        n: Mutex::new(0),
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConfigOptions {
    /// `- run: ...` / `- metric: "< x"` lines. Default: the three standard guardrails.
    pub guardrails: Option<Vec<String>>,
    pub protected_globs: Option<Vec<String>>,
    pub repeats: Option<u64>,
    pub run: Option<String>,
    pub confirm_trace: Option<String>,
    /// `evaluator.deps` (default: leave it to the schema default, auto).
    pub deps: Option<String>,
    /// `environment.image`
    pub environment_image: Option<String>,
}

/// The standard test config (legacy `bench:` section).
pub fn make_config(o: &ConfigOptions) -> RatchetConfig {
    let guardrails = o.guardrails.clone().unwrap_or_else(|| {
        vec![
            "- run: bash tests/check.sh".into(),
            "- error_rate: \"< 0.001\"".into(),
            "- peak_rss: \"< 1.2x\"".into(),
        ]
    });
    let globs = o
        .protected_globs
        .clone()
        .unwrap_or_else(|| vec!["bench/**".into(), "ratchet.yaml".into(), "tests/**".into()]);
    let run = o
        .run
        .clone()
        .unwrap_or_else(|| "bash bench/bench.sh".into());
    let yaml = format!(
        "version: 0
objective:
  metric: http_req_duration.p95
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: {}
  repeats: {}
  hardware: c7i.2xlarge
{}{}guardrails:{}
protected:{}
{}agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.2xlarge]
",
        serde_json::to_string(&run).unwrap_or_default(),
        o.repeats.unwrap_or(3),
        o.deps
            .as_ref()
            .map(|d| format!("  deps: {d}\n"))
            .unwrap_or_default(),
        o.confirm_trace
            .as_ref()
            .map(|t| format!("  confirm_trace: {t}\n"))
            .unwrap_or_default(),
        if guardrails.is_empty() {
            " []".to_owned()
        } else {
            format!(
                "\n{}",
                guardrails
                    .iter()
                    .map(|g| format!("  {g}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        },
        if globs.is_empty() {
            " []".to_owned()
        } else {
            format!(
                "\n{}",
                globs
                    .iter()
                    .map(|g| format!("  - \"{g}\""))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        },
        o.environment_image
            .as_ref()
            .map(|i| format!("environment:\n  image: {i}\n"))
            .unwrap_or_default(),
    );
    match ratchet_core::parse_config(&yaml) {
        Ok(c) => c,
        Err(e) => panic!("bad test config: {e:?}\n{yaml}"),
    }
}

/// Names of the entries of a directory (empty when it does not exist).
pub fn list_dir(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// In-memory `ArtifactStore` with access to its files.
#[derive(Default)]
pub struct MemoryStore {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl MemoryStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn get_text(&self, key: &str) -> Option<String> {
        self.files
            .lock()
            .ok()
            .and_then(|f| f.get(key).map(|b| String::from_utf8_lossy(b).into_owned()))
    }

    pub fn keys(&self) -> Vec<String> {
        self.files
            .lock()
            .map(|f| f.keys().cloned().collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl ArtifactStore for MemoryStore {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        if let Ok(mut f) = self.files.lock() {
            f.insert(key.to_owned(), body.to_vec());
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        Ok(self.get_text(key))
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        Ok(self
            .keys()
            .into_iter()
            .filter(|k| k.starts_with(prefix))
            .collect())
    }
}

type PutHook = Box<dyn Fn(&str, &Path) + Send + Sync>;

/// `ArtifactStore` that writes `<root>/<key>`; point `root` at `<s3Dir>/<bucket>` and the fake `aws s3 cp` finds the objects.
pub struct DirStore {
    root: PathBuf,
    puts: Mutex<Vec<String>>,
    after_put: Mutex<Option<PutHook>>,
}

impl DirStore {
    pub fn new(root: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(DirStore {
            root: root.into(),
            puts: Mutex::new(Vec::new()),
            after_put: Mutex::new(None),
        })
    }

    /// Keys written through `put`, in order.
    pub fn puts(&self) -> Vec<String> {
        self.puts.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Run `hook(key, path_of_the_object)` after every `put` (to tamper with what was uploaded).
    pub fn set_after_put(&self, hook: impl Fn(&str, &Path) + Send + Sync + 'static) {
        if let Ok(mut h) = self.after_put.lock() {
            *h = Some(Box::new(hook));
        }
    }
}

#[async_trait]
impl ArtifactStore for DirStore {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        let p = self.root.join(key);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| PortError::failed(e.to_string()))?;
        }
        std::fs::write(&p, body).map_err(|e| PortError::failed(e.to_string()))?;
        if let Ok(mut puts) = self.puts.lock() {
            puts.push(key.to_owned());
        }
        if let Ok(h) = self.after_put.lock()
            && let Some(h) = h.as_ref()
        {
            h(key, &p);
        }
        Ok(())
    }

    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        Ok(std::fs::read(self.root.join(key))
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    /// Everything under `root` (also objects the fake `aws s3 cp` uploaded), not just what `put` wrote.
    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        let mut out = Vec::new();
        walk_files(&self.root, "", &mut out);
        out.retain(|k| k.starts_with(prefix));
        out.sort();
        Ok(out)
    }
}

/// Relative paths of every file under `root`.
pub fn walk_files(root: &Path, rel: &str, out: &mut Vec<String>) {
    let dir = if rel.is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() {
            name
        } else {
            format!("{rel}/{name}")
        };
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk_files(root, &r, out);
        } else {
            out.push(r);
        }
    }
}

/// Is the process running? Zombies (killed, not yet reaped by a lazy init) count as dead.
pub fn alive(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rfind(')')
        .and_then(|i| stat.get(i + 2..i + 3))
        .is_none_or(|state| state != "Z")
}
