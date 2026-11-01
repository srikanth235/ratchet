//! Format checks for settings and secrets, shared by every service (port of
//! `packages/desktop/src/shared/live-settings.ts`). Pure: no regex crate, the patterns are small.

use super::common::FieldError;
use super::settings::{LiveSettingsInput, SecretName};

fn profile_ok(v: &str) -> bool {
    v.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | ':' | '-'))
}

/// `^[a-z]{2}(?:-[a-z]+)+-\d$`: `us-east-1`, `eu-west-2`, `ap-southeast-2`.
pub fn region_ok(v: &str) -> bool {
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

/// `^ami-(?:[0-9a-f]{8}|[0-9a-f]{17})$`
fn ami_ok(v: &str) -> bool {
    let Some(rest) = v.strip_prefix("ami-") else {
        return false;
    };
    (rest.len() == 8 || rest.len() == 17)
        && rest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// A path or a bare command name: no shell metacharacters, no control characters.
fn pathlike_ok(v: &str) -> bool {
    !v.is_empty()
        && !v.chars().any(|c| {
            c == '\0'
                || c == '\r'
                || c == '\n'
                || matches!(
                    c,
                    ';' | '&' | '|' | '`' | '$' | '<' | '>' | '*' | '?' | '"' | '\'' | '\\'
                )
        })
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/') || p.starts_with("~/") || {
        let b = p.as_bytes();
        b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'/' || b[2] == b'\\')
    }
}

/// `None` = valid. Empty values are valid (they mean "not set").
pub fn validate_live_setting(key: &str, raw: &str) -> Option<String> {
    let v = raw.trim();
    match key {
        "awsProfile" => {
            if v.chars().count() > 120 {
                Some("Profile names are at most 120 characters".into())
            } else if profile_ok(v) {
                None
            } else {
                Some("Profile names may only contain letters, digits and _ . @ : -".into())
            }
        }
        "awsRegion" => (!(v.is_empty() || region_ok(v)))
            .then(|| "Use a region id such as us-east-1 or eu-west-2".to_owned()),
        "runnerImageRef" | "benchImageRef" => (!(v.is_empty() || ami_ok(v)))
            .then(|| "Use an AMI id such as ami-0123456789abcdef0".to_owned()),
        "tofuBinary" => {
            if v.is_empty() {
                return None;
            }
            if v.chars().count() > 500 || !pathlike_ok(v) || v.starts_with('-') {
                return Some(
                    "Use a path or command name without spaces-as-arguments, quotes or shell characters"
                        .into(),
                );
            }
            (v.chars().any(char::is_whitespace) && !is_absolute(v))
                .then(|| "Use an absolute path (or just \"tofu\" for the one on PATH)".to_owned())
        }
        "moduleDir" => {
            if v.is_empty() {
                return None;
            }
            if v.chars().count() > 500 || !pathlike_ok(v) || v.starts_with('-') {
                return Some("Use a directory path without quotes or shell characters".into());
            }
            (!is_absolute(v)).then(|| "Use an absolute path".to_owned())
        }
        _ => Some("Unknown setting".into()),
    }
}

/// Per-field errors for every field present in `input` (empty = all valid).
pub fn validate_live_settings(input: &LiveSettingsInput) -> Vec<FieldError> {
    let fields: [(&str, &Option<String>); 6] = [
        ("awsProfile", &input.aws_profile),
        ("awsRegion", &input.aws_region),
        ("runnerImageRef", &input.runner_image_ref),
        ("benchImageRef", &input.bench_image_ref),
        ("tofuBinary", &input.tofu_binary),
        ("moduleDir", &input.module_dir),
    ];
    fields
        .into_iter()
        .filter_map(|(k, v)| {
            let v = v.as_deref()?;
            if v.chars().count() > 600 {
                return Some(FieldError::new(k, "Too long"));
            }
            validate_live_setting(k, v).map(|m| FieldError::new(k, m))
        })
        .collect()
}

/// A profile name for `PUT /v1/settings/aws-profile`.
pub fn validate_profile_name(v: &str) -> Option<String> {
    let v = v.trim();
    if v.chars().count() > 120 || !profile_ok(v) {
        Some("profile names may only contain letters, digits and _ . @ : - (at most 120)".into())
    } else {
        None
    }
}

fn github_token_ok(v: &str) -> bool {
    let alnum = |s: &str, lo: usize, hi: usize, extra_underscore: bool| {
        (lo..=hi).contains(&s.len())
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || (extra_underscore && c == '_'))
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
    fn regions() {
        for ok in ["us-east-1", "eu-west-2", "ap-southeast-2", "us-gov-west-1"] {
            assert!(region_ok(ok), "{ok}");
        }
        for bad in [
            "",
            "useast1",
            "us-east",
            "us-east-12",
            "US-east-1",
            "u-east-1",
        ] {
            assert!(!region_ok(bad), "{bad}");
        }
    }

    #[test]
    fn live_settings_are_checked_field_by_field() {
        let input = LiveSettingsInput {
            aws_region: Some("mars".into()),
            runner_image_ref: Some("ami-0123456789abcdef0".into()),
            bench_image_ref: Some("ami-xyz".into()),
            tofu_binary: Some("tofu; rm -rf /".into()),
            module_dir: Some("relative/dir".into()),
            ..Default::default()
        };
        let errors = validate_live_settings(&input);
        let paths: Vec<&str> = errors.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            ["awsRegion", "benchImageRef", "tofuBinary", "moduleDir"]
        );
        assert!(validate_live_settings(&LiveSettingsInput::default()).is_empty());
    }

    #[test]
    fn secret_formats() {
        let ant = "sk-ant-abcdefgh12345";
        assert_eq!(
            validate_secret_format(SecretName::AnthropicApiKey, ant),
            None
        );
        assert!(validate_secret_format(SecretName::OpenaiApiKey, ant).is_some());
        assert_eq!(
            validate_secret_format(SecretName::OpenaiApiKey, "sk-proj-abcdefghijklmnop"),
            None
        );
        assert!(
            validate_secret_format(SecretName::AnthropicApiKey, " sk-ant-abcdefgh12345").is_some()
        );
        let gh = format!("ghp_{}", "a".repeat(36));
        assert_eq!(validate_secret_format(SecretName::GithubToken, &gh), None);
        assert!(validate_secret_format(SecretName::GithubToken, "hunter2").is_some());
        assert_eq!(
            validate_secret_format(SecretName::GithubToken, &"0123456789abcdef".repeat(3)[..40]),
            None
        );
    }
}
