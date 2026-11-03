//! The AWS profiles and SSO sessions on this machine, parsed from `~/.aws/config` and `~/.aws/credentials`. Port of
//! `packages/desktop/src/main/aws-profiles.ts`.
//!
//! Names and kinds only: no key, token or account id is read into the result (the parser looks at key NAMES and, for
//! `sso_session`, the session name), so nothing here can leak a credential.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::api::{AwsProfileKind, AwsProfileSource, AwsProfileVm};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AwsProfileList {
    pub profiles: Vec<AwsProfileVm>,
    pub sso_sessions: Vec<String>,
    /// At least one of the files exists.
    pub files_found: bool,
}

#[derive(Debug, Clone, Default)]
pub struct AwsFiles {
    pub config: Option<String>,
    pub credentials: Option<String>,
}

struct Section {
    header: String,
    keys: BTreeMap<String, String>,
}

/// `[header]` sections with `key = value` lines; `#` and `;` comments; indented continuation lines are ignored.
fn sections(text: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            out.push(Section {
                header: line[1..line.len() - 1].trim().to_owned(),
                keys: BTreeMap::new(),
            });
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        // sub-settings of a nested block
        if raw.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(eq) = line.find('=').filter(|&i| i > 0) {
            cur.keys.insert(
                line[..eq].trim().to_lowercase(),
                line[eq + 1..].trim().to_owned(),
            );
        }
    }
    out
}

fn name_ok(s: &str) -> bool {
    (1..=120).contains(&s.chars().count())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | ':' | '-'))
}

fn kind_of(keys: &BTreeMap<String, String>, sso_session: bool) -> AwsProfileKind {
    if sso_session || keys.contains_key("sso_start_url") {
        AwsProfileKind::Sso
    } else if keys.contains_key("role_arn") {
        AwsProfileKind::Role
    } else if keys.contains_key("credential_process") {
        AwsProfileKind::Process
    } else if keys.contains_key("aws_access_key_id") {
        AwsProfileKind::Keys
    } else {
        AwsProfileKind::Other
    }
}

pub fn parse_aws_files(files: &AwsFiles) -> AwsProfileList {
    let mut by_name: BTreeMap<String, AwsProfileVm> = BTreeMap::new();
    let mut sso_sessions: BTreeSet<String> = BTreeSet::new();

    for s in sections(files.config.as_deref().unwrap_or("")) {
        if let Some(sess) = s
            .header
            .strip_prefix("sso-session")
            .filter(|r| r.starts_with(char::is_whitespace))
        {
            let sess = sess.trim();
            if name_ok(sess) {
                sso_sessions.insert(sess.to_owned());
            }
            continue;
        }
        let prof = if s.header == "default" {
            Some("default")
        } else {
            s.header
                .strip_prefix("profile")
                .filter(|r| r.starts_with(char::is_whitespace))
                .map(str::trim)
        };
        let Some(prof) = prof.filter(|p| name_ok(p)) else {
            continue;
        };
        let sso_session = s.keys.get("sso_session").cloned();
        let kind = kind_of(&s.keys, sso_session.is_some());
        by_name.insert(
            prof.to_owned(),
            AwsProfileVm {
                name: prof.to_owned(),
                kind,
                source: AwsProfileSource::Config,
                sso_session,
            },
        );
    }

    for s in sections(files.credentials.as_deref().unwrap_or("")) {
        if !name_ok(&s.header) {
            continue;
        }
        if let Some(existing) = by_name.get_mut(&s.header) {
            // Static keys in the credentials file win over an otherwise empty config section.
            if existing.kind == AwsProfileKind::Other && s.keys.contains_key("aws_access_key_id") {
                existing.kind = AwsProfileKind::Keys;
            }
            continue;
        }
        by_name.insert(
            s.header.clone(),
            AwsProfileVm {
                name: s.header.clone(),
                kind: if s.keys.contains_key("aws_access_key_id") {
                    AwsProfileKind::Keys
                } else {
                    AwsProfileKind::Other
                },
                source: AwsProfileSource::Credentials,
                sso_session: None,
            },
        );
    }

    let mut profiles: Vec<AwsProfileVm> = by_name.into_values().collect();
    // `default` first, then by name (localeCompare: case-insensitive first)
    profiles.sort_by(|a, b| match (a.name == "default", b.name == "default") {
        (true, _) => std::cmp::Ordering::Less,
        (_, true) => std::cmp::Ordering::Greater,
        _ => a
            .name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name)),
    });
    AwsProfileList {
        profiles,
        sso_sessions: sso_sessions.into_iter().collect(),
        files_found: files.config.is_some() || files.credentials.is_some(),
    }
}

/// Reads one file (the test seam of [`list_aws_profiles`]).
pub type ReadFile = Box<dyn Fn(&std::path::Path) -> std::io::Result<String>>;

#[derive(Default)]
pub struct ListAwsProfilesOptions {
    pub config_file_env: Option<String>,
    pub credentials_file_env: Option<String>,
    pub home: Option<PathBuf>,
    /// Test seam; default `std::fs::read_to_string`.
    pub read_file: Option<ReadFile>,
}

impl ListAwsProfilesOptions {
    /// From the process environment.
    pub fn from_env() -> Self {
        Self {
            config_file_env: std::env::var("AWS_CONFIG_FILE").ok(),
            credentials_file_env: std::env::var("AWS_SHARED_CREDENTIALS_FILE").ok(),
            ..Self::default()
        }
    }
}

/// Read the two files (honouring `AWS_CONFIG_FILE` / `AWS_SHARED_CREDENTIALS_FILE`) and parse them.
pub fn list_aws_profiles(opts: &ListAwsProfilesOptions) -> AwsProfileList {
    let home = opts
        .home
        .clone()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default();
    let try_read = |p: PathBuf| -> Option<String> {
        match &opts.read_file {
            Some(f) => f(&p).ok(),
            None => std::fs::read_to_string(p).ok(),
        }
    };
    let pick = |env: &Option<String>, default: &str| match env.as_deref() {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => home.join(".aws").join(default),
    };
    parse_aws_files(&AwsFiles {
        config: try_read(pick(&opts.config_file_env, "config")),
        credentials: try_read(pick(&opts.credentials_file_env, "credentials")),
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    const CONFIG: &str = r#"
# a comment
[default]
region = us-west-2

[profile dev-sso]
sso_session = corp
sso_account_id = 111122223333
sso_role_name = Admin
region = eu-west-1

[profile legacy-sso]
sso_start_url = https://corp.awsapps.com/start
sso_region = us-east-1

[profile deploy-role]
role_arn = arn:aws:iam::111122223333:role/Deploy
source_profile = default

[profile with-process]
credential_process = /usr/local/bin/get-creds

[profile "weird name"]
region = us-east-1

[profile static-in-config]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[sso-session corp]
sso_start_url = https://corp.awsapps.com/start
sso_region = us-east-1
sso_registration_scopes = sso:account:access
"#;

    const CREDENTIALS: &str = r"
[default]
aws_access_key_id = AKIAIOSFODNN7EXAMPLE
aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY

[ci-user]
aws_access_key_id = AKIAI44QH8DHBEXAMPLE
aws_secret_access_key = je7MtGbClwBF/2Zp9Utk/h3yCo8nvbEXAMPLEKEY
";

    /// machine.test.ts › AWS profiles from ~/.aws › lists profile and SSO session names with their kind, never a key
    #[test]
    fn lists_profile_and_sso_session_names_with_their_kind_never_a_key() {
        let r = parse_aws_files(&AwsFiles {
            config: Some(CONFIG.into()),
            credentials: Some(CREDENTIALS.into()),
        });
        assert!(r.files_found);
        assert_eq!(r.sso_sessions, ["corp"]);
        let got: Vec<_> = r
            .profiles
            .iter()
            .map(|p| (p.name.as_str(), p.kind))
            .collect();
        assert_eq!(
            got,
            [
                ("default", AwsProfileKind::Keys),
                ("ci-user", AwsProfileKind::Keys),
                ("deploy-role", AwsProfileKind::Role),
                ("dev-sso", AwsProfileKind::Sso),
                ("legacy-sso", AwsProfileKind::Sso),
                ("static-in-config", AwsProfileKind::Keys),
                ("with-process", AwsProfileKind::Process),
            ]
        );
        let dev = r.profiles.iter().find(|p| p.name == "dev-sso").unwrap();
        assert_eq!(dev.sso_session.as_deref(), Some("corp"));
        assert_eq!(dev.source, AwsProfileSource::Config);
        let ci = r.profiles.iter().find(|p| p.name == "ci-user").unwrap();
        assert_eq!(ci.source, AwsProfileSource::Credentials);
        let dump = serde_json::to_string(&r.profiles).unwrap();
        for secret in ["AKIA", "wJalr", "je7Mt", "111122223333", "awsapps.com"] {
            assert!(!dump.contains(secret), "{secret} leaked");
        }
    }

    /// machine.test.ts › AWS profiles from ~/.aws › handles missing files and empty ones
    #[test]
    fn handles_missing_files_and_empty_ones() {
        assert_eq!(
            parse_aws_files(&AwsFiles::default()),
            AwsProfileList::default()
        );
        let r = parse_aws_files(&AwsFiles {
            config: Some(String::new()),
            credentials: None,
        });
        assert!(r.files_found);
    }

    /// machine.test.ts › AWS profiles from ~/.aws › reads AWS_CONFIG_FILE / AWS_SHARED_CREDENTIALS_FILE, else ~/.aws
    #[test]
    fn reads_the_env_overrides_else_the_aws_directory() {
        let reads: Rc<RefCell<Vec<String>>> = Rc::default();
        let log = reads.clone();
        let r = list_aws_profiles(&ListAwsProfilesOptions {
            config_file_env: Some("/custom/config".into()),
            credentials_file_env: None,
            home: Some("/home/u".into()),
            read_file: Some(Box::new(move |p| {
                log.borrow_mut().push(p.display().to_string());
                if p.ends_with("custom/config") {
                    Ok("[profile x]\nregion=us-east-1\n".into())
                } else {
                    Err(std::io::Error::other("ENOENT"))
                }
            })),
        });
        let names: Vec<_> = r.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["x"]);
        assert_eq!(
            *reads.borrow(),
            ["/custom/config", "/home/u/.aws/credentials"]
        );
    }
}
