//! Harness detection: which agent CLIs are installed on this machine, and whether the user is signed in to them.
//!
//! Runs in the daemon (never the UI) on launch and on demand. Three things make it more than `which claude`:
//!  - PATH. A macOS GUI app inherits launchd's bare PATH, not the user's shell PATH, so `claude` / `codex` (installed by npm,
//!    Homebrew, nvm, volta, ...) are invisible. The PATH is resolved the way a login shell would (`$SHELL -lc`), merged with the
//!    process PATH and the usual install directories.
//!  - Auth. The CLIs' own non-interactive status command is asked first (`codex login status`, `claude auth status`; Grok has none);
//!    then the signals they use (credential files, the macOS keychain item), then API-key environment variables.
//!  - Secrets. Only the existence of credentials is ever checked. No credential file is opened and no key value is returned,
//!    logged or put in a hint (`security find-generic-password` runs without `-w` / `-g`, so the secret is not even fetched).
//!
//! Everything that touches the machine is injectable (spawner, fs, env, platform, home) and every probe has a timeout.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::process::{RunProcessOptions, SharedSpawner, TokioSpawner, run_process};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum HarnessKind {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "grok")]
    Grok,
}

impl HarnessKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude-code",
            HarnessKind::Codex => "codex",
            HarnessKind::Grok => "grok",
        }
    }

    pub fn agent_kind(self) -> ratchet_core::enums::AgentKind {
        match self {
            HarnessKind::ClaudeCode => ratchet_core::enums::AgentKind::ClaudeCode,
            HarnessKind::Codex => ratchet_core::enums::AgentKind::Codex,
            HarnessKind::Grok => ratchet_core::enums::AgentKind::Grok,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarnessAuth {
    #[serde(rename = "signed-in")]
    SignedIn,
    #[serde(rename = "api-key")]
    ApiKey,
    #[serde(rename = "signed-out")]
    SignedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessInfo {
    pub kind: HarnessKind,
    pub installed: bool,
    /// Parsed from `<binary> --version` (e.g. `2.1.4`); `None` when not installed or the version could not be read.
    pub version: Option<String>,
    /// Absolute path of the binary, `None` when not found.
    pub path: Option<String>,
    /// `signed-in`: the user's own login; `api-key`: no login but a key is set in the environment; else `signed-out`.
    pub auth: HarnessAuth,
    /// What to do next, or a one-line description of what was found. Never contains a credential.
    pub hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Darwin,
    Linux,
    Win32,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(windows) {
            Platform::Win32
        } else {
            Platform::Linux
        }
    }
}

/// The file system the probes use. Existence checks only: nothing here can read a file's contents.
pub trait DetectFs: Send + Sync {
    fn is_executable(&self, path: &str) -> bool;
    fn exists(&self, path: &str) -> bool;
    fn list_dir(&self, path: &str) -> Vec<String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NodeDetectFs;

impl DetectFs for NodeDetectFs {
    fn is_executable(&self, path: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }
    fn list_dir(&self, path: &str) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Default)]
pub struct DetectOptions {
    /// Runs the probes (`--version`, status commands, the login shell, `security`). Default: real processes.
    pub spawner: Option<SharedSpawner>,
    pub fs: Option<Arc<dyn DetectFs>>,
    /// Default: the process environment.
    pub env: Option<HashMap<String, String>>,
    pub platform: Option<Platform>,
    /// Default: `$HOME`, else the OS home directory.
    pub home: Option<String>,
    /// The login shell to ask for PATH. Default `env.SHELL`, else `/bin/zsh` on macOS and `/bin/sh` elsewhere.
    pub shell: Option<String>,
    /// Timeout of every probe except the login shell. Default 5 s.
    pub timeout: Option<Duration>,
    /// Timeout of the login-shell PATH probe (rc files can be slow). Default 8 s.
    pub shell_timeout: Option<Duration>,
    /// Which harnesses to look for. Default all three.
    pub kinds: Option<Vec<HarnessKind>>,
    /// A PATH already resolved by `resolve_login_path` (skips the login-shell probe).
    pub login_path: Option<String>,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5_000);
const DEFAULT_SHELL_TIMEOUT: Duration = Duration::from_millis(8_000);
const MARK: &str = "__RATCHET_PATH__";
const DELIMITER: char = ':';

struct Ctx {
    spawner: SharedSpawner,
    fs: Arc<dyn DetectFs>,
    env: HashMap<String, String>,
    platform: Platform,
    home: String,
    timeout: Duration,
    shell_timeout: Duration,
    shell: Option<String>,
}

fn home_dir() -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "/".to_string())
}

fn ctx_of(opts: &DetectOptions) -> Ctx {
    let env = opts
        .env
        .clone()
        .unwrap_or_else(|| std::env::vars().collect());
    let platform = opts.platform.unwrap_or_else(Platform::current);
    let shell = opts
        .shell
        .clone()
        .or_else(|| env.get("SHELL").cloned())
        .or(match platform {
            Platform::Darwin => Some("/bin/zsh".to_string()),
            Platform::Win32 => None,
            Platform::Linux => Some("/bin/sh".to_string()),
        });
    Ctx {
        spawner: opts
            .spawner
            .clone()
            .unwrap_or_else(|| Arc::new(TokioSpawner)),
        fs: opts.fs.clone().unwrap_or_else(|| Arc::new(NodeDetectFs)),
        home: opts
            .home
            .clone()
            .or_else(|| env.get("HOME").cloned())
            .unwrap_or_else(home_dir),
        env,
        platform,
        timeout: opts.timeout.unwrap_or(DEFAULT_TIMEOUT),
        shell_timeout: opts.shell_timeout.unwrap_or(DEFAULT_SHELL_TIMEOUT),
        shell,
    }
}

fn join(base: &str, parts: &[&str]) -> String {
    let mut p = PathBuf::from(base);
    for part in parts {
        p.push(part);
    }
    p.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------- probing

struct ProbeResult {
    /// Exited 0 within the timeout.
    ok: bool,
    code: Option<i32>,
    timed_out: bool,
    /// The process could not be started at all.
    spawn_failed: bool,
    stdout: String,
    stderr: String,
}

async fn probe(
    ctx: &Ctx,
    command: &str,
    args: &[&str],
    env: HashMap<String, String>,
    timeout: Duration,
) -> ProbeResult {
    let cancel = CancellationToken::new();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timer = {
        let (cancel, timed_out) = (cancel.clone(), timed_out.clone());
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            timed_out.store(true, Ordering::SeqCst);
            cancel.cancel();
        })
    };
    let mut lines: Vec<String> = Vec::new();
    let mut collect = |l: &str| lines.push(l.to_string());
    let mut options = RunProcessOptions::new(ctx.home.clone(), env);
    options.cancel = Some(&cancel);
    options.kill_grace = Duration::from_millis(500);
    options.on_stdout_line = Some(&mut collect);
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let res = run_process(ctx.spawner.as_ref(), command, &args, options).await;
    timer.abort();
    let timed_out = timed_out.load(Ordering::SeqCst);
    ProbeResult {
        ok: !timed_out && res.code() == Some(0),
        code: res.code(),
        timed_out,
        spawn_failed: res.spawn_error().is_some(),
        stdout: lines.join("\n"),
        stderr: res.stderr_tail,
    }
}

// ---------------------------------------------------------------- PATH

/// Directories where the CLIs usually live, that exist on this machine. Exported for tests.
pub fn fallback_path_dirs(
    fs: &dyn DetectFs,
    env: &HashMap<String, String>,
    home: &str,
) -> Vec<String> {
    let mut dirs: Vec<String> = vec![
        "/opt/homebrew/bin".into(),
        "/opt/homebrew/sbin".into(),
        "/usr/local/bin".into(),
        join(home, &[".local", "bin"]),
        join(home, &[".npm-global", "bin"]),
        join(home, &[".claude", "local"]),
        join(home, &[".volta", "bin"]),
        join(home, &[".asdf", "shims"]),
        join(home, &[".bun", "bin"]),
        join(home, &["Library", "pnpm"]),
        join(home, &[".local", "share", "pnpm"]),
    ];
    let set = |name: &str| env.get(name).filter(|v| !v.is_empty());
    if let Some(v) = set("NVM_BIN") {
        dirs.push(v.clone());
    }
    if let Some(v) = set("VOLTA_HOME") {
        dirs.push(join(v, &["bin"]));
    }
    if let Some(v) = set("PNPM_HOME") {
        dirs.push(v.clone());
    }
    if let Some(v) = set("ASDF_DATA_DIR") {
        dirs.push(join(v, &["shims"]));
    }
    if let Some(v) = set("BUN_INSTALL") {
        dirs.push(join(v, &["bin"]));
    }
    if let Some(v) = set("npm_config_prefix") {
        dirs.push(join(v, &["bin"]));
    }
    // nvm and fnm keep one directory per node version: newest first.
    let newest_first = |mut names: Vec<String>| {
        names.sort_by(|a, b| numeric_cmp(b, a));
        names
    };
    let nvm = join(home, &[".nvm", "versions", "node"]);
    for v in newest_first(fs.list_dir(&nvm)) {
        dirs.push(join(&nvm, &[&v, "bin"]));
    }
    let fnm = join(home, &[".fnm", "node-versions"]);
    for v in newest_first(fs.list_dir(&fnm)) {
        dirs.push(join(&fnm, &[&v, "installation", "bin"]));
    }
    dirs.push("/usr/bin".into());
    dirs.push("/bin".into());
    dirs.into_iter().filter(|d| fs.exists(d)).collect()
}

/// Locale-style comparison with numeric collation (`localeCompare(b, undefined, {numeric: true})`) for version directory names.
fn numeric_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, text)) if *d == digit => text.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let ord = if x.0 && y.0 {
            let (nx, ny) = (x.1.trim_start_matches('0'), y.1.trim_start_matches('0'));
            nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
        } else {
            x.1.to_lowercase().cmp(&y.1.to_lowercase())
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPath {
    /// Login-shell PATH, then the process PATH, then the usual install directories; no duplicates.
    pub path: String,
    /// Whether the login shell answered. When false only the process PATH and the fallbacks are in `path`.
    pub from_login_shell: bool,
}

fn parse_marked(stdout: &str) -> Option<String> {
    let end = stdout.rfind(MARK)?;
    let start = stdout[..end].rfind(MARK)?;
    let value = stdout[start + MARK.len()..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn split_path(path: &str) -> Vec<String> {
    path.split(DELIMITER)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// The PATH the user's terminal would have. Runs `$SHELL -lc 'echo $PATH'` (markers around the value, so noise that rc files
/// print is ignored; fish gets `string join :` because its `$PATH` is a list), with a timeout, and falls back to the process
/// PATH plus the common install directories when the shell is missing, slow or prints nothing.
pub async fn resolve_login_path(opts: &DetectOptions) -> LoginPath {
    let ctx = ctx_of(opts);
    let process_path = split_path(ctx.env.get("PATH").map(String::as_str).unwrap_or(""));
    let mut login: Vec<String> = Vec::new();
    if ctx.platform != Platform::Win32
        && let Some(shell) = &ctx.shell
    {
        let fish = Path::new(shell).file_name().is_some_and(|n| n == "fish");
        let script = if fish {
            format!("printf '%s%s%s' {MARK} (string join : $PATH) {MARK}")
        } else {
            format!("printf '%s' \"{MARK}$PATH{MARK}\"")
        };
        let res = probe(
            &ctx,
            shell,
            &["-lc", &script],
            ctx.env.clone(),
            ctx.shell_timeout,
        )
        .await;
        let value = if res.timed_out || res.spawn_failed {
            None
        } else {
            parse_marked(&res.stdout)
        };
        if let Some(value) = value {
            login = split_path(&value);
        }
    }
    let from_login_shell = !login.is_empty();
    let mut merged: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in login
        .into_iter()
        .chain(process_path)
        .chain(fallback_path_dirs(ctx.fs.as_ref(), &ctx.env, &ctx.home))
    {
        if seen.insert(dir.clone()) {
            merged.push(dir);
        }
    }
    LoginPath {
        path: merged.join(&DELIMITER.to_string()),
        from_login_shell,
    }
}

fn find_on_path(name: &str, path: &str, fs: &dyn DetectFs) -> Option<String> {
    path.split(DELIMITER)
        .filter(|d| !d.is_empty())
        .map(|dir| join(dir, &[name]))
        .find(|candidate| fs.is_executable(candidate))
}

// ---------------------------------------------------------------- version + auth

/// Leftmost match of `\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?`.
fn find_semver(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let digits = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    'outer: for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() {
            continue;
        }
        let mut pos = start;
        for part in 0..3 {
            let n = digits(pos);
            if n == 0 {
                continue 'outer;
            }
            pos += n;
            if part < 2 {
                if bytes.get(pos) != Some(&b'.') {
                    continue 'outer;
                }
                pos += 1;
            }
        }
        if matches!(bytes.get(pos), Some(b'-' | b'+')) {
            let n = bytes[pos + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'.' || **b == b'-')
                .count();
            if n > 0 {
                pos += 1 + n;
            }
        }
        return Some(&text[start..pos]);
    }
    None
}

pub fn parse_version(output: &str) -> Option<String> {
    let first = output
        .split('\n')
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if let Some(v) = find_semver(first).or_else(|| find_semver(output)) {
        return Some(v.to_string());
    }
    let head: String = first.trim().chars().take(40).collect();
    (!head.is_empty()).then_some(head)
}

/// Variables that would make a status command report the key instead of the stored login.
fn key_vars(kind: HarnessKind) -> &'static [&'static str] {
    match kind {
        HarnessKind::ClaudeCode => &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"],
        HarnessKind::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
        HarnessKind::Grok => &["XAI_API_KEY"],
    }
}

/// Model key variable(s) each harness uses when it runs on an API key.
pub fn model_key_vars(kind: HarnessKind) -> &'static [&'static str] {
    match kind {
        HarnessKind::ClaudeCode => &["ANTHROPIC_API_KEY"],
        HarnessKind::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY"],
        HarnessKind::Grok => &["XAI_API_KEY"],
    }
}

fn has_key(env: &HashMap<String, String>, kind: HarnessKind) -> bool {
    key_vars(kind)
        .iter()
        .any(|k| env.get(*k).is_some_and(|v| !v.is_empty()))
}

fn without_keys(env: &HashMap<String, String>, kind: HarnessKind) -> HashMap<String, String> {
    let mut out = env.clone();
    for k in key_vars(kind) {
        out.remove(*k);
    }
    out
}

struct AuthSignal {
    auth: HarnessAuth,
    via: &'static str,
}

fn signal(auth: HarnessAuth, via: &'static str) -> Option<AuthSignal> {
    Some(AuthSignal { auth, via })
}

fn parse_json_object(text: &str) -> Option<Map<String, Value>> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end < start {
        return None;
    }
    match serde_json::from_str::<Value>(&text[start..=end]) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn mentions_api_key(method: &str) -> bool {
    let m = method.to_lowercase();
    ["apikey", "api-key", "api_key", "api key"]
        .iter()
        .any(|p| m.contains(p))
}

async fn claude_auth(ctx: &Ctx, binary: &str, env: &HashMap<String, String>) -> Option<AuthSignal> {
    // 1. The CLI itself (`claude auth status` prints JSON and exits non-zero when signed out). Key variables are removed so the
    //    answer is about the stored login, not about the environment.
    let status = probe(
        ctx,
        binary,
        &["auth", "status"],
        without_keys(env, HarnessKind::ClaudeCode),
        ctx.timeout,
    )
    .await;
    if !status.spawn_failed
        && !status.timed_out
        && let Some(parsed) = parse_json_object(&status.stdout)
        && let Some(logged_in) = parsed.get("loggedIn").and_then(Value::as_bool)
    {
        if logged_in {
            let method = match parsed.get("authMethod") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
            };
            return if mentions_api_key(&method) {
                signal(HarnessAuth::ApiKey, "claude auth status")
            } else {
                signal(HarnessAuth::SignedIn, "claude auth status")
            };
        }
        // Definitive: the CLI says there is no login. An API key in the environment may still work.
        return if has_key(&ctx.env, HarnessKind::ClaudeCode) {
            signal(HarnessAuth::ApiKey, "ANTHROPIC_API_KEY")
        } else {
            signal(HarnessAuth::SignedOut, "claude auth status")
        };
    }
    // 2. Credential presence, the way the CLI stores them.
    let config_dir = ctx
        .env
        .get("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .cloned()
        .unwrap_or_else(|| join(&ctx.home, &[".claude"]));
    if ctx.fs.exists(&join(&config_dir, &[".credentials.json"])) {
        return signal(HarnessAuth::SignedIn, "credentials file");
    }
    if ctx.platform == Platform::Darwin {
        // Without -w / -g `security` prints the item's attributes and never fetches the secret (no keychain prompt).
        let kc = probe(
            ctx,
            "/usr/bin/security",
            &["find-generic-password", "-s", "Claude Code-credentials"],
            env.clone(),
            ctx.timeout,
        )
        .await;
        if kc.ok {
            return signal(HarnessAuth::SignedIn, "keychain");
        }
    }
    // 3. API key variables.
    if has_key(&ctx.env, HarnessKind::ClaudeCode) {
        return signal(HarnessAuth::ApiKey, "ANTHROPIC_API_KEY");
    }
    None
}

async fn codex_auth(ctx: &Ctx, binary: &str, env: &HashMap<String, String>) -> Option<AuthSignal> {
    // 1. `codex login status`: "Logged in using ChatGPT" / "API key configured - sk-***" / "Not logged in" (on stderr, exit 1).
    let status = probe(
        ctx,
        binary,
        &["login", "status"],
        without_keys(env, HarnessKind::Codex),
        ctx.timeout,
    )
    .await;
    if !status.spawn_failed && !status.timed_out {
        let text = format!("{}\n{}", status.stdout, status.stderr).to_lowercase();
        if text.contains("not logged in") {
            return if has_key(&ctx.env, HarnessKind::Codex) {
                signal(HarnessAuth::ApiKey, "OPENAI_API_KEY")
            } else {
                signal(HarnessAuth::SignedOut, "codex login status")
            };
        }
        if status.ok {
            if text.contains("chatgpt") {
                return signal(HarnessAuth::SignedIn, "codex login status");
            }
            if text.contains("api key") {
                return signal(HarnessAuth::ApiKey, "codex login status");
            }
            return signal(HarnessAuth::SignedIn, "codex login status");
        }
    }
    // 2. ~/.codex/auth.json present (its contents are not read, so ChatGPT login and stored key look the same).
    let home = ctx
        .env
        .get("CODEX_HOME")
        .filter(|d| !d.is_empty())
        .cloned()
        .unwrap_or_else(|| join(&ctx.home, &[".codex"]));
    if ctx.fs.exists(&join(&home, &["auth.json"])) {
        return signal(HarnessAuth::SignedIn, "auth.json");
    }
    if has_key(&ctx.env, HarnessKind::Codex) {
        return signal(HarnessAuth::ApiKey, "OPENAI_API_KEY");
    }
    None
}

/// Grok has no non-interactive status command, so the login is the credential file it keeps in `~/.grok` (existence only; the
/// token inside expires after a week and the CLI asks to sign in again), then `XAI_API_KEY`.
async fn grok_auth(ctx: &Ctx) -> Option<AuthSignal> {
    if ctx.fs.exists(&join(&ctx.home, &[".grok", "auth.json"])) {
        return signal(HarnessAuth::SignedIn, "auth.json");
    }
    if has_key(&ctx.env, HarnessKind::Grok) {
        return signal(HarnessAuth::ApiKey, "XAI_API_KEY");
    }
    None
}

// ---------------------------------------------------------------- per-harness

struct HarnessSpec {
    binary: &'static str,
    label: &'static str,
    install: &'static str,
    sign_in: &'static str,
}

fn spec_of(kind: HarnessKind) -> HarnessSpec {
    match kind {
        HarnessKind::ClaudeCode => HarnessSpec {
            binary: "claude",
            label: "Claude Code",
            install: "Install it with `npm install -g @anthropic-ai/claude-code` (see https://docs.claude.com/en/docs/claude-code/setup), then sign in with `claude`.",
            sign_in: "Run `claude` once and sign in (/login), or set ANTHROPIC_API_KEY.",
        },
        HarnessKind::Codex => HarnessSpec {
            binary: "codex",
            label: "Codex",
            install: "Install it with `npm install -g @openai/codex` or `brew install --cask codex`, then run `codex login`.",
            sign_in: "Run `codex login` (ChatGPT account or API key), or set OPENAI_API_KEY.",
        },
        HarnessKind::Grok => HarnessSpec {
            binary: "grok",
            label: "Grok",
            install: "Install it with `npm install -g @xai-official/grok`, then run `grok login`.",
            sign_in: "Run `grok login` (grok.com account), or set XAI_API_KEY.",
        },
    }
}

fn auth_note(auth: HarnessAuth, label: &str, via: &str) -> String {
    match auth {
        HarnessAuth::SignedIn => {
            format!("{label} is signed in on this machine ({via}); agents use your own login.")
        }
        HarnessAuth::ApiKey => format!("{label} will use the API key from {via}."),
        HarnessAuth::SignedOut => format!("{label} is not signed in."),
    }
}

async fn detect_one(kind: HarnessKind, ctx: &Ctx, search_path: &str) -> HarnessInfo {
    let spec = spec_of(kind);
    let Some(path) = find_on_path(spec.binary, search_path, ctx.fs.as_ref()) else {
        return HarnessInfo {
            kind,
            installed: false,
            version: None,
            path: None,
            auth: HarnessAuth::SignedOut,
            hint: format!("{} was not found on PATH. {}", spec.label, spec.install),
        };
    };
    let mut env = ctx.env.clone();
    env.insert("PATH".to_string(), search_path.to_string());
    // The shebang of an npm-installed CLI is `#!/usr/bin/env node`: it needs the resolved PATH too.
    let v = probe(ctx, &path, &["--version"], env.clone(), ctx.timeout).await;
    if !v.ok {
        let why = if v.timed_out {
            "timed out".to_string()
        } else if v.spawn_failed {
            format!(
                "cannot run it ({})",
                if v.stderr.is_empty() {
                    "spawn failed"
                } else {
                    &v.stderr
                }
            )
        } else {
            let code = v.code.map_or("null".to_string(), |c| c.to_string());
            let first = v.stderr.split('\n').next().unwrap_or("");
            format!(
                "exited with {code}{}",
                if v.stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {first}")
                }
            )
        };
        return HarnessInfo {
            kind,
            installed: false,
            version: None,
            hint: format!(
                "Found {} at {path} but `--version` {why}. Reinstall it. {}",
                spec.label, spec.install
            ),
            path: Some(path),
            auth: HarnessAuth::SignedOut,
        };
    }
    let version = parse_version(&v.stdout);
    let found = match kind {
        HarnessKind::ClaudeCode => claude_auth(ctx, &path, &env).await,
        HarnessKind::Codex => codex_auth(ctx, &path, &env).await,
        HarnessKind::Grok => grok_auth(ctx).await,
    };
    let auth = found.as_ref().map_or(HarnessAuth::SignedOut, |s| s.auth);
    let hint = match &found {
        Some(s) if auth != HarnessAuth::SignedOut => auth_note(auth, spec.label, s.via),
        _ => format!(
            "{} {}",
            auth_note(HarnessAuth::SignedOut, spec.label, ""),
            spec.sign_in
        ),
    };
    HarnessInfo {
        kind,
        installed: true,
        version,
        path: Some(path),
        auth,
        hint,
    }
}

/// Probe this machine for the Claude Code, Codex and Grok CLIs. Never fails: a probe that fails or times out yields a result with a
/// hint. Results come back in a stable order (claude-code, codex, grok).
pub async fn detect_harnesses(opts: &DetectOptions) -> Vec<HarnessInfo> {
    let ctx = ctx_of(opts);
    let search_path = match &opts.login_path {
        Some(p) => p.clone(),
        None => resolve_login_path(opts).await.path,
    };
    let kinds = opts.kinds.clone().unwrap_or_else(|| {
        vec![
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Grok,
        ]
    });
    let order = [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::Grok,
    ];
    let wanted: Vec<HarnessKind> = order.into_iter().filter(|k| kinds.contains(k)).collect();
    futures::future::join_all(
        wanted
            .into_iter()
            .map(|k| detect_one(k, &ctx, &search_path)),
    )
    .await
}

// ---------------------------------------------------------------- environment for local agents

/// `(^|_)(API_?KEY|TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIALS?|AUTH)(_|$)|^AWS_|^AZURE_|^GOOGLE_APPLICATION_CREDENTIALS$|^NPM_CONFIG__AUTH`
/// (case-insensitive): variables that carry credentials the agents must not inherit from the app: cloud, git hosting, registries,
/// model keys.
fn is_secret_name(name: &str) -> bool {
    let upper = name.to_uppercase();
    if upper.starts_with("AWS_")
        || upper.starts_with("AZURE_")
        || upper == "GOOGLE_APPLICATION_CREDENTIALS"
        || upper.starts_with("NPM_CONFIG__AUTH")
    {
        return true;
    }
    const WORDS: [&str; 9] = [
        "APIKEY",
        "API_KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "CREDENTIALS",
        "AUTH",
    ];
    WORDS.iter().any(|word| {
        upper.match_indices(word).any(|(i, _)| {
            let before_ok = i == 0 || upper.as_bytes()[i - 1] == b'_';
            let after = i + word.len();
            let after_ok = after == upper.len() || upper.as_bytes()[after] == b'_';
            before_ok && after_ok
        })
    })
}

/// The environment a local agent process gets: the user's own environment (HOME, so the CLI finds its login) with the login-shell
/// PATH, minus anything that looks like a credential (AWS, GitHub and registry tokens, every `*_API_KEY`). The model key of the
/// harness is added back only when it reports auth `api-key` and the key is set; with a `signed-in` harness no key is passed, so
/// the CLI uses the user's login rather than an API key that happens to be in the environment.
pub fn local_agent_env(
    kind: HarnessKind,
    auth: HarnessAuth,
    base: &HashMap<String, String>,
    path: &str,
) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = base
        .iter()
        .filter(|(k, _)| !is_secret_name(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.insert("PATH".to_string(), path.to_string());
    if auth == HarnessAuth::ApiKey {
        for k in model_key_vars(kind) {
            if let Some(v) = base.get(*k).filter(|v| !v.is_empty()) {
                env.insert((*k).to_string(), v.clone());
            }
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_scan() {
        assert_eq!(find_semver("2.1.4 (Claude Code)"), Some("2.1.4"));
        assert_eq!(find_semver("codex-cli 0.46.0"), Some("0.46.0"));
        assert_eq!(find_semver("v1.2.3-beta.1 extra"), Some("1.2.3-beta.1"));
        assert_eq!(find_semver("1.2.3-"), Some("1.2.3"));
        assert_eq!(find_semver("1.2"), None);
    }

    #[test]
    fn numeric_directory_order() {
        let mut v = vec!["v18.9.0".to_string(), "v22.1.0".into(), "v20.10.0".into()];
        v.sort_by(|a, b| numeric_cmp(b, a));
        assert_eq!(v, ["v22.1.0", "v20.10.0", "v18.9.0"]);
    }

    #[test]
    fn secret_names() {
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "AWS_PROFILE",
            "NPM_TOKEN",
            "SSH_AUTH_SOCK",
            "DB_PASSWORD",
            "MY_APIKEY",
            "token",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ] {
            assert!(is_secret_name(name), "{name}");
        }
        for name in [
            "HOME",
            "PATH",
            "LANG",
            "HTTPS_PROXY",
            "AUTHOR_NAME",
            "TOKENIZERS_PARALLELISM",
            "KEYBOARD",
        ] {
            assert!(!is_secret_name(name), "{name}");
        }
    }
}
