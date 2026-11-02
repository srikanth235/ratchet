//! The chat engine on this Mac: the user's own Claude Code, Codex or Grok, a read-only checkout per chat.
//!
//! Access is fixed when the session opens (`Access::ReadOnly`, the adapters turn it into each CLI's own flags: no file edits, no
//! shell) and nothing ever asks a human mid-chat. The checkout is a `git worktree` of Ratchet's own clone of the repo (never the
//! user's working copy); a Home chat with no repo gets an empty directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use ratchet_agents::detect::{HarnessAuth as DetectAuth, HarnessKind as DetectKind};
use ratchet_agents::git::{Git, exec_git};
use ratchet_agents::models::{ModelCatalog, ModelUse};
use ratchet_agents::workdir::{
    AddWorktreeOptions, Identity, LocalCloneOptions, add_worktree, ensure_local_clone,
    git_auth_env, remove_worktree,
};
use ratchet_agents::{LocalAdapterOptions, local_harness_for};
use ratchet_core::ports::{LiveSession, PortError, PortResult, SessionSpec, SharedEventSink};
use ratchet_engine::desktop::HarnessCache;

use crate::api::{HarnessAuth, HarnessKind};
use crate::chat::{ChatEngine, ChatHarness};

/// Where a repo is cloned from. The default is `https://github.com/<slug>.git`; tests point it at a local repository.
pub type CloneUrl = Arc<dyn Fn(&str) -> String + Send + Sync>;
/// The GitHub token git authenticates with (read when a checkout is made, never stored here).
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct CliChatOptions {
    pub harnesses: Arc<HarnessCache>,
    pub models: ModelCatalog,
    /// Clones and worktrees live under here (`<root>/clones`, `<root>/worktrees`, `<root>/scratch`).
    pub root: PathBuf,
    pub clone_url: Option<CloneUrl>,
    pub github_token: Option<TokenSource>,
    /// How the adapters are built (options per CLI, a fake process spawner in tests). Detection results and the login PATH are
    /// filled in when a session opens.
    pub adapters: LocalAdapterOptions,
}

pub struct CliChatEngine {
    harnesses: Arc<HarnessCache>,
    models: ModelCatalog,
    root: PathBuf,
    clone_url: CloneUrl,
    github_token: TokenSource,
    adapters: LocalAdapterOptions,
    /// One clone operation at a time per machine: two chats on the same repo must not clone into the same directory.
    clone_lock: tokio::sync::Mutex<()>,
}

const ORDER: [(DetectKind, HarnessKind); 3] = [
    (DetectKind::ClaudeCode, HarnessKind::ClaudeCode),
    (DetectKind::Codex, HarnessKind::Codex),
    (DetectKind::Grok, HarnessKind::Grok),
];

impl CliChatEngine {
    pub fn new(o: CliChatOptions) -> Self {
        Self {
            harnesses: o.harnesses,
            models: o.models,
            root: o.root,
            clone_url: o
                .clone_url
                .unwrap_or_else(|| Arc::new(|slug| format!("https://github.com/{slug}.git"))),
            github_token: o.github_token.unwrap_or_else(|| Arc::new(|| None)),
            adapters: o.adapters,
            clone_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn clone_dir(&self, slug: &str) -> PathBuf {
        self.root.join("clones").join(slug.replace('/', "__"))
    }

    fn worktree_dir(&self, chat_id: &str) -> PathBuf {
        self.root.join("worktrees").join(chat_id)
    }

    fn git_env(&self, url: &str) -> Option<HashMap<String, String>> {
        let token = (self.github_token)()?;
        let env = git_auth_env(url, &token);
        (!env.is_empty()).then_some(env)
    }

    /// The clone's default branch: what `origin/HEAD` points at, else `main`, else `master`.
    async fn default_branch(clone: &Git) -> Option<String> {
        if let Ok(head) = clone
            .run(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .await
            && let Some(b) = head.trim().strip_prefix("origin/")
        {
            return Some(b.to_owned());
        }
        for b in ["main", "master"] {
            if clone
                .rev_parse(&format!("refs/remotes/origin/{b}"))
                .await
                .is_some()
            {
                return Some(b.to_owned());
            }
        }
        None
    }

    fn chat_branch(chat_id: &str) -> String {
        format!("ratchet-chat/{chat_id}")
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_owned()
}

#[async_trait]
impl ChatEngine for CliChatEngine {
    async fn harnesses(&self) -> Vec<ChatHarness> {
        let detected = self.harnesses.get().await;
        ORDER
            .iter()
            .map(|(dk, hk)| {
                let info = detected.harnesses.iter().find(|h| h.kind == *dk);
                let ready = info.is_some_and(|h| h.installed && h.auth != DetectAuth::SignedOut);
                ChatHarness {
                    kind: *hk,
                    ready,
                    auth: match info.map(|h| h.auth) {
                        Some(DetectAuth::SignedIn) => HarnessAuth::SignedIn,
                        Some(DetectAuth::ApiKey) => HarnessAuth::ApiKey,
                        _ => HarnessAuth::SignedOut,
                    },
                    hint: info.map_or_else(
                        || format!("{} was not found on this Mac.", hk.label()),
                        |h| h.hint.clone(),
                    ),
                    default_model: self
                        .models
                        .default_for(*dk, ModelUse::Chat)
                        .map(|m| m.id.clone()),
                }
            })
            .collect()
    }

    async fn prepare_workdir(&self, chat_id: &str, repo: Option<&str>) -> PortResult<PathBuf> {
        let Some(slug) = repo else {
            let dir = self.root.join("scratch").join(chat_id);
            tokio::fs::create_dir_all(&dir).await?;
            return Ok(dir);
        };
        let url = (self.clone_url)(slug);
        let env = self.git_env(&url);
        let clone = {
            let _guard = self.clone_lock.lock().await;
            ensure_local_clone(LocalCloneOptions {
                repo_url: url,
                dir: self.clone_dir(slug),
                git: None,
                env: env.clone(),
            })
            .await
            .map_err(|e| PortError::failed(first_line(&e.to_string())))?
        };
        let base = Self::default_branch(&clone)
            .await
            .ok_or_else(|| PortError::failed(format!("{slug} has no default branch to read")))?;
        let dir = self.worktree_dir(chat_id);
        add_worktree(AddWorktreeOptions {
            clone,
            dir: dir.clone(),
            branch: Self::chat_branch(chat_id),
            base_ref: base,
            identity: Identity {
                name: "Ratchet chat".into(),
                email: "chat@ratchet.invalid".into(),
            },
            git: None,
            env,
        })
        .await
        .map_err(|e| PortError::failed(first_line(&e.to_string())))?;
        Ok(dir)
    }

    async fn release_workdir(&self, chat_id: &str) {
        let scratch = self.root.join("scratch").join(chat_id);
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        let dir = self.worktree_dir(chat_id);
        if !dir.exists() {
            return;
        }
        // the clone is the worktree's `.git` parent: find it from the worktree itself
        let git = Git::new(&dir, exec_git(), None);
        let common = git
            .run(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .await
            .ok()
            .map(|p| PathBuf::from(p.trim()));
        let clone_dir = common
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        match clone_dir {
            Some(c) => {
                let clone = Git::new(&c, exec_git(), None);
                remove_worktree(&clone, &dir).await;
                let _ = clone
                    .try_run(&["branch", "-D", &Self::chat_branch(chat_id)])
                    .await;
            }
            None => {
                let _ = tokio::fs::remove_dir_all(&dir).await;
            }
        }
    }

    async fn open(
        &self,
        kind: HarnessKind,
        spec: SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let detected = self.harnesses.get().await;
        let opts = LocalAdapterOptions {
            harnesses: detected.harnesses,
            login_path: detected.login_path.unwrap_or_default(),
            ..self.adapters.clone()
        };
        let harness = local_harness_for(&opts, kind.agent_kind()).map_err(PortError::failed)?;
        harness.open(&spec, events).await
    }
}
