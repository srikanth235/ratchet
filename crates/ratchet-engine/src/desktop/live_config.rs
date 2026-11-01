//! What the live runtime is configured with: where OpenTofu and its module live, and where the engine reads secrets
//! from. Port of `packages/desktop/src/main/live-config.ts`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratchet_connectors::{AgentConnectorSource, ConnectorAgent, ResolvedConnections};
use ratchet_core::enums::AgentKind;

/// What empty `tofu_binary` / `module_dir` settings resolve to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivePaths {
    pub tofu_binary: String,
    pub module_dir: String,
}

impl Default for LivePaths {
    fn default() -> Self {
        Self {
            tofu_binary: "tofu".into(),
            module_dir: "infra/aws".into(),
        }
    }
}

pub struct DefaultPathsInput {
    pub is_packaged: bool,
    pub resources_path: PathBuf,
    pub repo_root: PathBuf,
    /// Test seam; default: the filesystem.
    pub exists: Option<Box<dyn Fn(&Path) -> bool>>,
}

/// Packaged: the module ships in `Resources/infra/aws` and a pinned `tofu` in `Resources/bin/tofu` (when the build includes
/// one); from a checkout: `<repo>/infra/aws` and `tofu` from PATH.
pub fn default_live_paths(o: &DefaultPathsInput) -> LivePaths {
    let exists = |p: &Path| o.exists.as_ref().map_or_else(|| p.exists(), |f| f(p));
    let bundled_tofu = o.resources_path.join("bin").join("tofu");
    LivePaths {
        module_dir: if o.is_packaged {
            o.resources_path.join("infra").join("aws")
        } else {
            o.repo_root.join("infra").join("aws")
        }
        .display()
        .to_string(),
        tofu_binary: if o.is_packaged && exists(&bundled_tofu) {
            bundled_tofu.display().to_string()
        } else {
            "tofu".into()
        },
    }
}

/// Empty settings fall back to the defaults.
pub fn effective_paths(tofu_binary: &str, module_dir: &str, defaults: &LivePaths) -> LivePaths {
    LivePaths {
        tofu_binary: if tofu_binary.is_empty() {
            defaults.tofu_binary.clone()
        } else {
            tofu_binary.to_owned()
        },
        module_dir: if module_dir.is_empty() {
            defaults.module_dir.clone()
        } else {
            module_dir.to_owned()
        },
    }
}

/// The setting a runner secret name (`ANTHROPIC_API_KEY`, ...) comes from.
pub fn setting_for_secret_env(name: &str) -> Option<&'static str> {
    match name {
        "ANTHROPIC_API_KEY" => Some("anthropicApiKey"),
        "OPENAI_API_KEY" => Some("openaiApiKey"),
        "GITHUB_TOKEN" => Some("githubToken"),
        _ => None,
    }
}

/// Looks a secret up by the setting's name (`anthropicApiKey`, ...): the store or the environment.
pub type SecretLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The engine's secret source: `(agent kind, runner secret name) -> value`. Resolves in the daemon only; the engine hands
/// the value to the runner's parameter store and nowhere else. The GitHub token and the API keys come from the connectors
/// chosen for the repo; a model key falls back to the environment (`env`, by setting name) only when no connector of that
/// agent holds one (never next to a chosen login).
pub fn secret_source(
    resolved: &ResolvedConnections,
    env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> Arc<dyn Fn(AgentKind, &str) -> Option<String> + Send + Sync> {
    let token = resolved.github.as_ref().map(|g| g.token.clone());
    let keys = resolved.cloud_keys.clone();
    let logins: Vec<ConnectorAgent> = resolved
        .agents
        .iter()
        .filter(|a| a.source == AgentConnectorSource::Login)
        .map(|a| a.agent)
        .collect();
    Arc::new(move |_kind, name| {
        if name == "GITHUB_TOKEN" {
            return token.clone();
        }
        let agent = match name {
            "ANTHROPIC_API_KEY" => ConnectorAgent::ClaudeCode,
            "OPENAI_API_KEY" => ConnectorAgent::Codex,
            _ => return None,
        };
        if let Some(k) = keys.get(&agent) {
            return Some(k.clone());
        }
        if logins.contains(&agent) {
            return None;
        }
        setting_for_secret_env(name).and_then(&env)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(packaged: bool, tofu_exists: bool) -> DefaultPathsInput {
        DefaultPathsInput {
            is_packaged: packaged,
            resources_path: "/App/Resources".into(),
            repo_root: "/repo".into(),
            exists: Some(Box::new(move |_| tofu_exists)),
        }
    }

    #[test]
    fn a_checkout_uses_the_repo_module_and_tofu_from_path() {
        let p = default_live_paths(&input(false, true));
        assert_eq!(p.module_dir, "/repo/infra/aws");
        assert_eq!(p.tofu_binary, "tofu");
    }

    #[test]
    fn a_packaged_app_uses_its_resources_and_the_bundled_tofu_when_present() {
        let p = default_live_paths(&input(true, true));
        assert_eq!(p.module_dir, "/App/Resources/infra/aws");
        assert_eq!(p.tofu_binary, "/App/Resources/bin/tofu");
        assert_eq!(default_live_paths(&input(true, false)).tofu_binary, "tofu");
    }

    #[test]
    fn empty_settings_fall_back_to_the_defaults() {
        let d = LivePaths::default();
        assert_eq!(effective_paths("", "", &d), d);
        let p = effective_paths("/opt/tofu", "/m", &d);
        assert_eq!(
            (p.tofu_binary.as_str(), p.module_dir.as_str()),
            ("/opt/tofu", "/m")
        );
    }

    fn resolved(
        token: Option<&str>,
        keys: &[(ConnectorAgent, &str)],
        logins: &[ConnectorAgent],
    ) -> ResolvedConnections {
        use ratchet_connectors::{ResolvedAgent, ResolvedGithub};
        ResolvedConnections {
            github: token.map(|t| ResolvedGithub {
                id: "g".into(),
                name: "g".into(),
                login: None,
                token: t.into(),
            }),
            aws: None,
            agents: logins
                .iter()
                .map(|a| ResolvedAgent {
                    connector_id: format!("login-{a:?}"),
                    name: "login".into(),
                    agent: *a,
                    source: AgentConnectorSource::Login,
                    where_: ratchet_core::enums::AgentsWhere::Local,
                    api_key: None,
                })
                .collect(),
            cloud_keys: keys.iter().map(|(a, k)| (*a, (*k).to_owned())).collect(),
        }
    }

    /// live-config.test.ts › the secret source: the GitHub token and the keys come from the chosen connectors; the
    /// environment only fills in a key when no connector holds one and no login was chosen for that agent
    #[test]
    fn the_secret_source_maps_runner_names_to_the_chosen_connectors() {
        let env = |k: &str| (k == "openaiApiKey").then(|| "sk-env".to_owned());
        let src = secret_source(
            &resolved(
                Some("ghp_x"),
                &[(ConnectorAgent::ClaudeCode, "sk-ant-x")],
                &[],
            ),
            env,
        );
        assert_eq!(
            src(AgentKind::ClaudeCode, "GITHUB_TOKEN").as_deref(),
            Some("ghp_x")
        );
        assert_eq!(
            src(AgentKind::ClaudeCode, "ANTHROPIC_API_KEY").as_deref(),
            Some("sk-ant-x")
        );
        // no connector holds an OpenAI key: the environment's
        assert_eq!(
            src(AgentKind::Codex, "OPENAI_API_KEY").as_deref(),
            Some("sk-env")
        );
        assert_eq!(src(AgentKind::Codex, "SOMETHING_ELSE"), None);

        // a chosen login never gets a key from the environment
        let src = secret_source(&resolved(None, &[], &[ConnectorAgent::Codex]), env);
        assert_eq!(src(AgentKind::Codex, "OPENAI_API_KEY"), None);
        assert_eq!(src(AgentKind::ClaudeCode, "GITHUB_TOKEN"), None);
    }
}
