//! Repo slugs and the small text helpers around them (`repo-slug.ts`, and `githubSlug` from `local-repo.ts`).

use regex::Regex;

/// `https://github.com/acme/api.git`, `acme/api`, `git@github.com:acme/api` -> `acme/api`. A filesystem path is not a
/// repo slug.
pub fn slug_of(repo: &str) -> Option<String> {
    let re = Regex::new(
        r"^(?:(?:https?://|ssh://git@|git@)(?:[^/:]+)[/:])?([A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9._-]+?)(?:\.git)?/?$",
    )
    .expect("static regex");
    let repo = repo.trim();
    if repo.starts_with('/') {
        return None;
    }
    re.captures(repo).map(|m| m[1].to_string())
}

/// `https://github.com/acme/api.git`, `git@github.com:acme/api`, `ssh://git@github.com/acme/api` -> `acme/api`.
/// Only github.com remotes count.
pub fn github_slug(remote: &str) -> Option<String> {
    let re =
        Regex::new(r"github\.com[/:]([A-Za-z0-9][A-Za-z0-9-]*)/([A-Za-z0-9._-]+?)(?:\.git)?/?$")
            .expect("static regex");
    re.captures(remote.trim())
        .map(|m| format!("{}/{}", &m[1], &m[2]))
}

/// The runtime's own wording mentions screens that no longer exist; say what to do in the new model.
pub fn friendly_missing(missing: &[String]) -> Vec<String> {
    missing
        .iter()
        .map(|m| {
            if m.contains("GitHub repository and token") {
                "Sign in with GitHub (Settings)".to_string()
            } else if m.contains("bench AMI") {
                "The evaluator image for this app version was not found in your AWS account (connect AWS in Settings, or build the image)".to_string()
            } else if m.contains("runner AMI") {
                "The cloud runner image was not found in your AWS account (use \"This Mac\", or build the image)".to_string()
            } else if m.contains("install and sign in to Claude Code or Codex") {
                "Install and sign in to Claude Code or Codex on this machine (Settings > Agents)".to_string()
            } else {
                m.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // product-engine.test.ts: slugOf
    #[test]
    fn slug_of_cases() {
        for (repo, slug) in [
            ("acme/api", Some("acme/api")),
            ("https://github.com/acme/api.git", Some("acme/api")),
            ("git@github.com:acme/api", Some("acme/api")),
            ("/work/search/remote.git", None),
            ("C:\\x\\y", None),
        ] {
            assert_eq!(slug_of(repo).as_deref(), slug, "{repo}");
        }
    }

    // machine.test.ts: githubSlug
    #[test]
    fn github_slug_cases() {
        for (remote, slug) in [
            ("https://github.com/acme/api.git", Some("acme/api")),
            ("https://github.com/acme/api", Some("acme/api")),
            ("git@github.com:acme/api.git", Some("acme/api")),
            ("ssh://git@github.com/acme/api.git", Some("acme/api")),
            (
                "https://x-access-token:abc@github.com/acme/api.git",
                Some("acme/api"),
            ),
            ("https://gitlab.com/acme/api.git", None),
            ("/srv/repos/api.git", None),
        ] {
            assert_eq!(github_slug(remote).as_deref(), slug, "{remote}");
        }
    }

    // product-engine.test.ts: friendlyMissing (the wording asserted by "says what is missing when the runtime is not ready")
    #[test]
    fn friendly_missing_rewrites_known_lines_and_keeps_others() {
        let out = friendly_missing(&[
            "set the bench AMI (Settings)".into(),
            "install and sign in to Claude Code or Codex on this machine (Settings > Agents)"
                .into(),
            "needs a GitHub repository and token".into(),
            "set the runner AMI".into(),
        ]);
        assert!(out[0].starts_with("The evaluator image for this app version was not found"));
        assert!(out[1].starts_with("Install and sign in to Claude Code or Codex"));
        assert_eq!(out[2], "Sign in with GitHub (Settings)");
        assert!(out[3].starts_with("The cloud runner image was not found"));
        assert_eq!(
            friendly_missing(&["something else".into()]),
            ["something else"]
        );
    }
}
