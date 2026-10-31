//! Format checks for what a connector stores, ported from `packages/desktop/src/shared/live-settings.ts`
//! (`validateLiveSetting` for the AWS profile and region, `validateSecretFormat`). Pure: no regex, the patterns are small.

/// The three secrets of the first versions of Settings, by the name they were stored under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SecretName {
    AnthropicApiKey,
    OpenaiApiKey,
    GithubToken,
}

impl SecretName {
    /// The account name in the secret store and the setting's name in the old settings file.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AnthropicApiKey => "anthropicApiKey",
            Self::OpenaiApiKey => "openaiApiKey",
            Self::GithubToken => "githubToken",
        }
    }

    /// The environment variables that count as the secret being provided, in order of preference.
    pub fn env_vars(self) -> &'static [&'static str] {
        match self {
            Self::AnthropicApiKey => &["ANTHROPIC_API_KEY"],
            Self::OpenaiApiKey => &["OPENAI_API_KEY"],
            Self::GithubToken => &["GITHUB_TOKEN", "GH_TOKEN"],
        }
    }
}

/// `us-east-1`, `eu-west-2`, `ap-southeast-2`: `^[a-z]{2}(?:-[a-z]+)+-\d$`.
fn region_ok(v: &str) -> bool {
    let parts: Vec<&str> = v.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    let first = parts[0];
    let last = parts[parts.len() - 1];
    first.len() == 2
        && first.chars().all(|c| c.is_ascii_lowercase())
        && last.len() == 1
        && last.chars().all(|c| c.is_ascii_digit())
        && parts[1..parts.len() - 1]
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_lowercase()))
}

/// `None` = valid. Empty is valid here (it means "not set"); the caller refuses an empty profile.
pub fn validate_aws_profile(raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.chars().count() > 120 {
        return Some("Profile names are at most 120 characters".into());
    }
    let ok = v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | ':' | '-'));
    (!ok).then(|| "Profile names may only contain letters, digits and _ . @ : -".to_owned())
}

/// `None` = valid. Empty is valid here (it means "use the default").
pub fn validate_aws_region(raw: &str) -> Option<String> {
    let v = raw.trim();
    (!(v.is_empty() || region_ok(v)))
        .then(|| "Use a region id such as us-east-1 or eu-west-2".to_owned())
}

fn github_token_ok(v: &str) -> bool {
    let alnum = |s: &str, lo: usize, hi: usize, underscore: bool| {
        (lo..=hi).contains(&s.len())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || (underscore && c == '_'))
    };
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        if let Some(rest) = v.strip_prefix(p) {
            return alnum(rest, 20, 255, false);
        }
    }
    if let Some(rest) = v.strip_prefix("github_pat_") {
        return alnum(rest, 20, 255, true);
    }
    v.len() == 40
        && v.chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Loose prefix checks that catch the common paste mistakes (wrong key in the wrong box, stray whitespace).
pub fn validate_secret_format(name: SecretName, value: &str) -> Option<String> {
    if value != value.trim() || value.chars().any(char::is_whitespace) {
        return Some("Remove spaces and line breaks around the value".into());
    }
    let key_chars = |s: &str, min: usize| {
        s.len() >= min
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    match name {
        SecretName::AnthropicApiKey => value
            .strip_prefix("sk-ant-")
            .is_none_or(|r| !key_chars(r, 8))
            .then(|| "Anthropic keys start with sk-ant-".to_owned()),
        SecretName::OpenaiApiKey => (value.starts_with("sk-ant-")
            || value.strip_prefix("sk-").is_none_or(|r| !key_chars(r, 16)))
        .then(|| {
            "OpenAI keys start with sk- (not sk-ant-) and are at least 20 characters".to_owned()
        }),
        SecretName::GithubToken => (!github_token_ok(value))
            .then(|| "GitHub tokens start with ghp_, gho_, ghu_, ghs_ or github_pat_".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_and_profiles() {
        for ok in ["us-east-1", "eu-west-2", "ap-southeast-2", ""] {
            assert_eq!(validate_aws_region(ok), None, "{ok}");
        }
        for bad in [
            "useast1",
            "us-east",
            "us-east-12",
            "US-east-1",
            "mars-north-9",
        ] {
            assert!(validate_aws_region(bad).is_some(), "{bad}");
        }
        assert_eq!(validate_aws_profile("acme-prod"), None);
        assert!(validate_aws_profile("bad profile; rm -rf /").is_some());
        assert!(validate_aws_profile(&"x".repeat(121)).is_some());
    }

    #[test]
    fn secret_formats() {
        let ant = "sk-ant-abcdefgh12345";
        assert_eq!(
            validate_secret_format(SecretName::AnthropicApiKey, ant),
            None
        );
        assert!(validate_secret_format(SecretName::OpenaiApiKey, ant).is_some());
        assert!(validate_secret_format(SecretName::GithubToken, "not-a-token").is_some());
        let gh = format!("ghp_{}", "a".repeat(36));
        assert_eq!(validate_secret_format(SecretName::GithubToken, &gh), None);
        assert!(validate_secret_format(SecretName::GithubToken, &format!(" {gh}")).is_some());
    }
}
