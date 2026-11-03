//! `packages/runtime/src/live-checks.test.ts`

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_engine::runtime::live_checks::{
    ConnectionResult, ConnectionTarget, ConnectionTesterDeps, HttpProbe, create_connection_tester,
};
use ratchet_github::GitHubAccess;
use ratchet_sandbox::CallerIdentity;
use ratchet_sandbox::sandbox::{ProcessResult, ProcessRunner, ProcessSpec};

const KEY: &str = "sk-ant-api03-VERYSECRETKEYVALUE";

/// `reply(status)`: answers every GET with `status` and records `(url, headers)`.
struct Reply {
    status: Option<u16>,
    error: Option<String>,
    calls: Mutex<Vec<(String, Vec<(String, String)>)>>,
}

impl Reply {
    fn status(status: u16) -> Arc<Self> {
        Arc::new(Self {
            status: Some(status),
            error: None,
            calls: Mutex::default(),
        })
    }
}

#[async_trait]
impl HttpProbe for Reply {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, String)],
        _t: Duration,
    ) -> Result<u16, String> {
        self.calls.lock().unwrap().push((
            url.to_owned(),
            headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        ));
        match (&self.error, self.status) {
            (Some(e), _) => Err(e.clone()),
            (None, Some(s)) => Ok(s),
            _ => unreachable!(),
        }
    }
}

fn r(target: ConnectionTarget, ok: bool, detail: &str) -> ConnectionResult {
    ConnectionResult {
        target,
        ok,
        detail: detail.to_owned(),
    }
}

fn probe(p: &Arc<Reply>) -> ConnectionTesterDeps {
    ConnectionTesterDeps {
        fetch: Some(p.clone()),
        ..Default::default()
    }
}

#[tokio::test]
async fn aws_reports_the_account_passes_region_and_profile_through_and_reports_credential_errors() {
    let seen: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::default();
    let s2 = seen.clone();
    let t = create_connection_tester(ConnectionTesterDeps {
        check_aws: Some(Arc::new(move |o| {
            s2.lock()
                .unwrap()
                .push((o.region.clone(), o.profile.clone()));
            Box::pin(async {
                Ok(CallerIdentity {
                    account_id: "123456789012".into(),
                    arn: "arn".into(),
                })
            })
        })),
        ..Default::default()
    });
    assert_eq!(
        t.aws("eu-west-1", Some("sandbox"), None).await,
        r(
            ConnectionTarget::Aws,
            true,
            "signed in to account 123456789012 in eu-west-1 as profile sandbox"
        )
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [("eu-west-1".to_owned(), Some("sandbox".to_owned()))]
    );

    let expired = create_connection_tester(ConnectionTesterDeps {
        check_aws: Some(Arc::new(|_| {
            Box::pin(async {
                Err("The SSO session associated with this profile has expired\nstack...".to_owned())
            })
        })),
        ..Default::default()
    });
    assert_eq!(
        expired.aws("us-east-1", Some("sso"), None).await,
        r(
            ConnectionTarget::Aws,
            false,
            "The SSO session associated with this profile has expired"
        )
    );
}

#[tokio::test]
async fn github_requires_push_rights_and_never_leaks_the_token() {
    let access = |private, can_push| GitHubAccess {
        full_name: "acme/api".into(),
        default_branch: "main".into(),
        private,
        can_push,
    };
    let with = |a: GitHubAccess| ConnectionTesterDeps {
        check_github: Some(Arc::new(move |_| {
            let a = a.clone();
            Box::pin(async move { Ok(a) })
        })),
        ..Default::default()
    };
    let ok = create_connection_tester(with(access(true, true)));
    let res = ok.github("acme/api", "ghp_x1234567890").await;
    assert!(
        res.ok
            && res
                .detail
                .contains("acme/api (private, default branch main)"),
        "{res:?}"
    );
    let ro = create_connection_tester(with(access(false, false)));
    let res = ro.github("acme/api", "ghp_x1234567890").await;
    assert!(!res.ok && res.detail.contains("cannot push"), "{res:?}");
    let leaky = create_connection_tester(ConnectionTesterDeps {
        check_github: Some(Arc::new(|_| {
            Box::pin(async { Err("bad credentials ghp_x1234567890".to_owned()) })
        })),
        ..Default::default()
    });
    let res = leaky.github("acme/api", "ghp_x1234567890").await;
    assert!(!res.ok);
    assert!(!res.detail.contains("ghp_x1234567890"), "{res:?}");
}

#[tokio::test]
async fn anthropic_openai_probe_the_models_endpoint_with_the_key_in_a_header_and_map_statuses() {
    let f = Reply::status(200);
    let t = create_connection_tester(probe(&f));
    assert_eq!(
        t.anthropic(KEY).await,
        r(
            ConnectionTarget::Anthropic,
            true,
            "Anthropic accepted the key"
        )
    );
    {
        let calls = f.calls.lock().unwrap();
        let (url, headers) = &calls[0];
        assert!(
            url.starts_with("https://api.anthropic.com/v1/models"),
            "{url}"
        );
        assert!(headers.iter().any(|(k, v)| k == "x-api-key" && v == KEY));
        assert!(!url.contains(KEY));
    }
    t.openai("sk-proj-abcdefghij").await;
    {
        let calls = f.calls.lock().unwrap();
        let (url, headers) = &calls[1];
        assert_eq!(url, "https://api.openai.com/v1/models");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "authorization" && v == "Bearer sk-proj-abcdefghij")
        );
    }
    let a = create_connection_tester(probe(&Reply::status(401)))
        .anthropic(KEY)
        .await;
    assert!(!a.ok);
    assert_eq!(a.detail, "Anthropic rejected the key (401)");
    let o = create_connection_tester(probe(&Reply::status(500)))
        .openai("sk-proj-abcdefghij")
        .await;
    assert!(!o.ok);
    assert_eq!(o.detail, "OpenAI answered 500");
    assert!(
        create_connection_tester(probe(&Reply::status(429)))
            .openai("sk-proj-abcdefghij")
            .await
            .ok
    );
}

#[tokio::test]
async fn network_errors_are_reported_without_the_key() {
    let f = Arc::new(Reply {
        status: None,
        error: Some(format!(
            "getaddrinfo ENOTFOUND api.anthropic.com (key {KEY})"
        )),
        calls: Mutex::default(),
    });
    let res = create_connection_tester(probe(&f)).anthropic(KEY).await;
    assert!(!res.ok);
    assert!(res.detail.contains("ENOTFOUND"));
    assert!(!res.detail.contains(KEY));
}

struct Runner(Result<(String, i32), String>);

#[async_trait]
impl ProcessRunner for Runner {
    async fn run(&self, _spec: &ProcessSpec) -> ratchet_sandbox::Result<ProcessResult> {
        match &self.0 {
            Ok((stdout, code)) => Ok(ProcessResult {
                exit_code: *code,
                stdout: stdout.clone(),
                stderr: String::new(),
            }),
            Err(e) => Err(ratchet_sandbox::Error::other(e.clone())),
        }
    }
}

fn exists(yes: bool) -> ratchet_engine::runtime::live_checks::PathExists {
    Arc::new(move |_: &Path| Box::pin(async move { yes }))
}

fn tofu_deps(runner: Runner, yes: bool) -> ConnectionTesterDeps {
    ConnectionTesterDeps {
        process_runner: Some(Arc::new(runner)),
        exists: Some(exists(yes)),
        ..Default::default()
    }
}

#[tokio::test]
async fn tofu_finds_the_module_and_an_opentofu_binary() {
    let ok = create_connection_tester(tofu_deps(
        Runner(Ok(("OpenTofu v1.8.2\non linux_amd64\n".into(), 0))),
        true,
    ));
    assert_eq!(
        ok.tofu("tofu", Path::new("/m/infra/aws")).await,
        r(
            ConnectionTarget::Tofu,
            true,
            "OpenTofu 1.8.2, module /m/infra/aws"
        )
    );

    let no_module =
        create_connection_tester(tofu_deps(Runner(Ok(("OpenTofu v1.8.2".into(), 0))), false));
    let res = no_module.tofu("tofu", Path::new("/nope")).await;
    assert!(
        !res.ok && res.detail.contains("main.tf not found"),
        "{res:?}"
    );

    let terraform =
        create_connection_tester(tofu_deps(Runner(Ok(("Terraform v1.5.7".into(), 0))), true));
    let res = terraform.tofu("terraform", Path::new("/m")).await;
    assert!(!res.ok && res.detail.contains("not OpenTofu"), "{res:?}");

    let missing =
        create_connection_tester(tofu_deps(Runner(Err("spawn tofu ENOENT".into())), true));
    let res = missing.tofu("tofu", Path::new("/m")).await;
    assert!(!res.ok && res.detail.contains("cannot run tofu"), "{res:?}");
}
