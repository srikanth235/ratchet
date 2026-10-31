//! Port of `packages/desktop/src/main/github.test.ts`: the GitHub REST client (against the fake GitHub server instead of an
//! injected `fetch`) and the device-flow sign-in loop. Plus tests for the additions (branches, `inspect_github_repo`,
//! `MockGithubApi`).

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::ports::{CancellationToken, Clock, PortError, PortResult};
use ratchet_github::testing::{
    DEVICE_CODE, FAKE_LOGIN, FAKE_TOKEN, FakeGithubConfig, FakeGithubServer, repo, test_client,
};
use ratchet_github::{
    AccountStore, DeviceCode, DevicePoll, FlowState, GithubApi, GithubApiOptions, GithubSignIn,
    HttpGithubApi, MockGithubApi, PLACEHOLDER_GITHUB_CLIENT_ID, Repo, SignInDeps,
    inspect_github_repo,
};
use serde_json::{Value, json};

fn api_for(server: &FakeGithubServer, client_id: &str) -> HttpGithubApi {
    HttpGithubApi::new(GithubApiOptions {
        client_id: Some(client_id.into()),
        api_url: Some(server.base_url.clone()),
        web_url: Some(server.base_url.clone()),
        http: Some(test_client()),
    })
}

async fn empty_server() -> FakeGithubServer {
    FakeGithubServer::start(FakeGithubConfig::default()).await
}

// ------------------------------------------------------------------ GitHub REST

#[tokio::test]
async fn starts_the_device_flow_with_the_client_id_and_the_repo_scope_and_parses_the_code() {
    let server = empty_server().await;
    let api = api_for(&server, "Iv1.mine");
    assert!(api.client_id_configured());
    let code = api.device_start().await.unwrap();
    assert_eq!(
        code,
        DeviceCode {
            device_code: "dc-e2e".into(),
            user_code: DEVICE_CODE.into(),
            verification_uri: format!("{}/login/device", server.base_url),
            expires_in: 900,
            interval: 1,
        }
    );
    let form = &server.forms()[0];
    assert_eq!(form["client_id"], "Iv1.mine");
    assert_eq!(form["scope"], "repo");
    assert_eq!(server.requests(), ["POST /login/device/code"]);
}

#[test]
fn knows_the_placeholder_client_id_is_not_a_real_one() {
    let with = |client_id: Option<&str>| {
        HttpGithubApi::new(GithubApiOptions {
            client_id: client_id.map(String::from),
            ..Default::default()
        })
    };
    assert!(!with(None).client_id_configured());
    assert!(!with(Some("  ")).client_id_configured());
    assert!(!with(Some(PLACEHOLDER_GITHUB_CLIENT_ID)).client_id_configured());
    assert!(with(Some("Iv1.real")).client_id_configured());
    assert!(PLACEHOLDER_GITHUB_CLIENT_ID.contains("Placeholder"));
}

#[tokio::test]
async fn maps_the_device_poll_answers() {
    let cases: Vec<(Value, DevicePoll)> = vec![
        (
            json!({ "access_token": "gho_TOKENTOKENTOKEN1234567890" }),
            DevicePoll::Ok {
                token: "gho_TOKENTOKENTOKEN1234567890".into(),
            },
        ),
        (
            json!({ "error": "authorization_pending" }),
            DevicePoll::Pending,
        ),
        (
            json!({ "error": "slow_down", "interval": 12 }),
            DevicePoll::SlowDown { interval: 12 },
        ),
        (json!({ "error": "expired_token" }), DevicePoll::Expired),
        (json!({ "error": "access_denied" }), DevicePoll::Denied),
        (
            json!({ "error": "incorrect_client_credentials", "error_description": "bad client" }),
            DevicePoll::Error {
                message: "bad client".into(),
            },
        ),
    ];
    for (answer, expected) in cases {
        let server = empty_server().await;
        server.script_poll(answer.clone());
        let got = api_for(&server, "x").device_poll("dc").await.unwrap();
        assert_eq!(got, expected, "{answer}");
        let form = &server.forms()[0];
        assert_eq!(
            form["grant_type"],
            "urn:ietf:params:oauth:grant-type:device_code"
        );
        assert_eq!(form["device_code"], "dc");
        assert_eq!(form["client_id"], "x");
    }
}

#[tokio::test]
async fn reads_the_account_lists_repos_most_recently_pushed_first_across_pages_and_filters() {
    let page = |n: usize, from: usize| -> Vec<Value> {
        (0..n)
            .map(|i| {
                let mut r = repo(
                    &format!("acme/r{}", from + i),
                    "main",
                    if i == 0 { Some("search service") } else { None },
                    i % 2 == 0,
                );
                r["pushed_at"] = json!("2026-01-01T00:00:00Z");
                r
            })
            .collect()
    };
    let repos: Vec<Value> = page(100, 0).into_iter().chain(page(3, 100)).collect();
    let server = FakeGithubServer::start(FakeGithubConfig {
        repos,
        ..Default::default()
    })
    .await;
    let api = api_for(&server, "x");

    assert_eq!(api.user("tok").await.unwrap().login, FAKE_LOGIN);
    assert_eq!(server.authorizations()[0], "tok");

    let all = api.repos("tok", None).await.unwrap();
    assert_eq!(all.len(), 103);
    assert_eq!(
        all[0],
        Repo {
            full_name: "acme/r0".into(),
            default_branch: "main".into(),
            private: true,
            description: Some("search service".into()),
            pushed_at: Some("2026-01-01T00:00:00Z".into()),
        }
    );
    let names = |v: Vec<Repo>| v.into_iter().map(|r| r.full_name).collect::<Vec<_>>();
    assert_eq!(
        names(api.repos("tok", Some("SEARCH")).await.unwrap()),
        ["acme/r0", "acme/r100"]
    );
    assert!(
        names(api.repos("tok", Some("acme/r10")).await.unwrap()).contains(&"acme/r101".to_string())
    );
    // two pages were enough for 103 repos
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.as_str() == "GET /user/repos")
            .count(),
        6
    );
}

#[tokio::test]
async fn reads_a_file_at_a_ref_base64_and_returns_none_for_a_missing_one() {
    let mut config = FakeGithubConfig::default();
    config
        .files
        .insert("acme/api@main:ratchet.yaml".into(), "version: 0\n".into());
    config
        .files
        .insert("acme/api@main:dir/with space/é.txt".into(), "odd".into());
    let server = FakeGithubServer::start(config).await;
    let api = api_for(&server, "x");
    assert_eq!(
        api.file("tok", "acme/api", "main", "ratchet.yaml")
            .await
            .unwrap()
            .as_deref(),
        Some("version: 0\n")
    );
    assert_eq!(
        api.file("tok", "acme/api", "main", "dir/with space/é.txt")
            .await
            .unwrap()
            .as_deref(),
        Some("odd")
    );
    assert_eq!(
        api.file("tok", "acme/api", "main", "nope.yaml")
            .await
            .unwrap(),
        None
    );
    // the ref matters
    assert_eq!(
        api.file("tok", "acme/api", "other", "ratchet.yaml")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn turns_http_failures_into_plain_messages_that_never_carry_the_token() {
    let secret = "gho_SECRETSECRETSECRET";
    let server = empty_server().await;
    let api = api_for(&server, "x");
    server.fail_next_request(401, "Bad credentials");
    let e = api.user(secret).await.unwrap_err();
    assert!(e.to_string().contains("sign in again"), "{e}");
    let e = api.repo(secret, "acme/x").await.unwrap_err();
    assert!(e.to_string().contains("not found"), "{e}");
    server.fail_next_request(403, "rate limited");
    let e = api.repos(secret, None).await.unwrap_err();
    assert!(
        e.to_string().contains("403") && e.to_string().contains("rate limit"),
        "{e}"
    );

    // a transport failure (nothing listens there) must not echo the token either
    let dead = HttpGithubApi::new(GithubApiOptions {
        api_url: Some("http://127.0.0.1:1".into()),
        http: Some(test_client()),
        ..Default::default()
    });
    let e = dead.user(secret).await.unwrap_err();
    assert!(e.to_string().starts_with("Could not reach GitHub"), "{e}");
    assert!(!e.to_string().contains("SECRETSECRET"), "{e}");
}

// ------------------------------------------------------------------ additions: branches, inspect, mock

#[tokio::test]
async fn lists_branches_and_reads_ratchet_yaml_from_the_default_branch() {
    let mut config = FakeGithubConfig {
        repos: vec![
            repo("acme/api", "trunk", Some("checkout service"), true),
            repo("acme/bare", "main", None, false),
        ],
        ..Default::default()
    };
    config
        .branches
        .insert("acme/api".into(), vec!["trunk".into(), "feature".into()]);
    config
        .files
        .insert("acme/api@trunk:ratchet.yaml".into(), "version: 0\n".into());
    let server = FakeGithubServer::start(config).await;
    let api = api_for(&server, "x");
    assert_eq!(
        api.branches("tok", "acme/api").await.unwrap(),
        ["trunk", "feature"]
    );
    assert!(
        api.branches("tok", "acme/none")
            .await
            .unwrap_err()
            .to_string()
            .contains("not found")
    );

    let found = inspect_github_repo(&api, "tok", "acme/api").await.unwrap();
    assert_eq!(found.repo.default_branch, "trunk");
    assert_eq!(found.config_yaml.as_deref(), Some("version: 0\n"));

    // a repo without ratchet.yaml still inspects, with no config
    let bare = inspect_github_repo(&api, "tok", "acme/bare").await.unwrap();
    assert_eq!(bare.config_yaml, None);
    // a failure reading the file surfaces instead of being read as "no config"
    server.fail_next_request(403, "nope");
    assert!(inspect_github_repo(&api, "tok", "acme/api").await.is_err());
}

#[tokio::test]
async fn the_mock_api_behaves_like_the_ts_fake() {
    let mock = MockGithubApi::new();
    assert_eq!(mock.repos("t", None).await.unwrap().len(), 2);
    // the query is lower-cased, then matched against the full name
    assert_eq!(
        mock.repos("t", Some("WEB"))
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.full_name)
            .collect::<Vec<_>>(),
        ["acme/web"]
    );
    assert_eq!(
        mock.repo("t", "acme/web").await.unwrap().default_branch,
        "trunk"
    );
    assert!(
        mock.repo("t", "acme/zzz")
            .await
            .unwrap_err()
            .to_string()
            .contains("not found")
    );
    mock.with(|s| {
        s.files
            .insert("acme/api@main:ratchet.yaml".into(), "v".into());
        s.polls.push_back(DevicePoll::Denied);
    });
    assert_eq!(
        mock.file("t", "acme/api", "main", "ratchet.yaml")
            .await
            .unwrap()
            .as_deref(),
        Some("v")
    );
    assert_eq!(mock.device_poll("dc").await.unwrap(), DevicePoll::Denied);
    assert_eq!(mock.device_poll("dc").await.unwrap(), DevicePoll::Pending);
    assert_eq!(mock.branches("t", "acme/web").await.unwrap(), ["trunk"]);
    mock.with(|s| s.user_fails = true);
    assert!(mock.user("t").await.is_err());
    assert_eq!(mock.with(|s| s.tokens_seen.len()), 5);
}

// ------------------------------------------------------------------ the sign-in loop

#[derive(Default)]
struct TestStore {
    token: Mutex<Option<String>>,
    account: Mutex<Option<(String, String)>>,
}

impl AccountStore for TestStore {
    fn set_token(&self, token: &str) {
        *self.token.lock().unwrap() = Some(token.into());
    }
    fn set_account(&self, login: &str, source: &str) {
        *self.account.lock().unwrap() = Some((login.into(), source.into()));
    }
}

/// How a [`TestClock`] sleeps.
enum Sleep {
    /// Returns at once.
    Instant,
    /// Waits for the cancellation, then reports it.
    UntilCancelled,
    /// A real (short) pause.
    Real(Duration),
}

/// The clock of the sign-in tests: records every sleep and moves `now` by `step` ms per sleep.
struct TestClock {
    t: AtomicI64,
    step: i64,
    sleeps: Mutex<Vec<u64>>,
    mode: Sleep,
}

impl TestClock {
    fn new(mode: Sleep, step: i64) -> Arc<Self> {
        Arc::new(TestClock {
            t: AtomicI64::new(0),
            step,
            sleeps: Mutex::default(),
            mode,
        })
    }

    fn instant() -> Arc<Self> {
        Self::new(Sleep::Instant, 0)
    }

    fn sleeps(&self) -> Vec<u64> {
        self.sleeps.lock().unwrap().clone()
    }
}

#[async_trait]
impl Clock for TestClock {
    fn now(&self) -> i64 {
        self.t.load(Ordering::SeqCst)
    }

    async fn sleep(&self, ms: u64, cancel: Option<&CancellationToken>) -> PortResult<()> {
        self.sleeps.lock().unwrap().push(ms);
        self.t.fetch_add(self.step, Ordering::SeqCst);
        match &self.mode {
            Sleep::Instant => Ok(()),
            Sleep::UntilCancelled => {
                if let Some(c) = cancel {
                    c.cancelled().await;
                }
                Err(PortError::Cancelled)
            }
            Sleep::Real(d) => {
                tokio::time::sleep(*d).await;
                Ok(())
            }
        }
    }
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..400 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Let background work run to a standstill: a short fixed wait, like the TS `settle`.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(60)).await;
}

#[tokio::test]
async fn sign_in_shows_the_code_opens_the_browser_polls_until_approved_then_keeps_the_token() {
    let api = Arc::new(MockGithubApi::new());
    api.with(|s| {
        s.polls = [
            DevicePoll::Pending,
            DevicePoll::SlowDown { interval: 10 },
            DevicePoll::Ok {
                token: "gho_DEVICEFLOWTOKEN1234567890".into(),
            },
        ]
        .into();
        s.login = "octocat".into();
    });
    let store = Arc::new(TestStore::default());
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let clock = TestClock::instant();
    let signed_in = Arc::new(AtomicUsize::new(0));

    let mut deps = SignInDeps::new(api.clone(), store.clone());
    let o = opened.clone();
    deps.open_external = Some(Arc::new(move |u| o.lock().unwrap().push(u.to_string())));
    deps.clock = clock.clone();
    let si = signed_in.clone();
    deps.on_signed_in = Some(Arc::new(move || {
        si.fetch_add(1, Ordering::SeqCst);
    }));
    let flow = GithubSignIn::new(deps);

    let f = flow.start().await.unwrap();
    assert_eq!(f.state, FlowState::Pending);
    assert_eq!(f.user_code, "ABCD-1234");
    assert_eq!(f.verification_uri, "https://github.com/login/device");
    assert_eq!(*opened.lock().unwrap(), ["https://github.com/login/device"]);

    wait_until("sign-in", || signed_in.load(Ordering::SeqCst) == 1).await;
    assert_eq!(
        store.token.lock().unwrap().as_deref(),
        Some("gho_DEVICEFLOWTOKEN1234567890")
    );
    assert_eq!(
        *store.account.lock().unwrap(),
        Some(("octocat".to_string(), "device".to_string()))
    );
    assert_eq!(flow.current(), None);
    // the interval GitHub asked for (5 s), then slowed down (+5 s at least)
    assert_eq!(clock.sleeps()[..3], [5000, 5000, 10_000]);
    assert!(!serde_json::to_string(&f).unwrap().contains("gho_"));
}

#[tokio::test]
async fn sign_in_ends_with_a_message_and_stores_nothing() {
    let cases = [
        (DevicePoll::Denied, FlowState::Denied, "denied"),
        (DevicePoll::Expired, FlowState::Expired, "expired"),
        (
            DevicePoll::Error {
                message: "bad client id".into(),
            },
            FlowState::Error,
            "bad client id",
        ),
    ];
    for (poll, state, msg) in cases {
        let api = Arc::new(MockGithubApi::new());
        api.with(|s| s.polls = [poll.clone()].into());
        let store = Arc::new(TestStore::default());
        let mut deps = SignInDeps::new(api, store.clone());
        deps.clock = TestClock::instant();
        let flow = GithubSignIn::new(deps);
        flow.start().await.unwrap();
        wait_until("terminal state", || {
            flow.current()
                .is_some_and(|f| f.state != FlowState::Pending)
        })
        .await;
        let cur = flow.current().unwrap();
        assert_eq!(cur.state, state, "{poll:?}");
        assert!(cur.message.unwrap().contains(msg));
        assert!(store.token.lock().unwrap().is_none());
    }
}

#[tokio::test]
async fn sign_in_expires_on_its_own_when_the_code_outlives_its_time() {
    let api = Arc::new(MockGithubApi::new());
    let store = Arc::new(TestStore::default());
    let mut deps = SignInDeps::new(api, store);
    // every sleep jumps 1000 s ahead: the code (900 s) has expired by the first poll
    deps.clock = TestClock::new(Sleep::Instant, 1_000_000);
    let flow = GithubSignIn::new(deps);
    flow.start().await.unwrap();
    wait_until("expiry", || {
        flow.current()
            .is_some_and(|f| f.state == FlowState::Expired)
    })
    .await;
}

#[tokio::test]
async fn sign_in_cancel_stops_the_loop_and_a_second_start_replaces_the_first_flow() {
    let api = Arc::new(MockGithubApi::new());
    let store = Arc::new(TestStore::default());
    let mut deps = SignInDeps::new(api.clone(), store.clone());
    // sleeps until cancelled
    deps.clock = TestClock::new(Sleep::UntilCancelled, 0);
    let flow = GithubSignIn::new(deps);
    flow.start().await.unwrap();
    flow.cancel();
    assert_eq!(flow.current(), None);
    flow.start().await.unwrap();
    flow.start().await.unwrap();
    assert_eq!(api.with(|s| s.started), 3);
    flow.cancel();
    settle().await;
    assert!(store.token.lock().unwrap().is_none());
    assert_eq!(flow.current(), None);
}

#[tokio::test]
async fn sign_in_end_to_end_against_the_fake_server() {
    let server = empty_server().await;
    let api: Arc<dyn GithubApi> = Arc::new(api_for(&server, "e2e-client-id"));
    let store = Arc::new(TestStore::default());
    let mut deps = SignInDeps::new(api, store.clone());
    deps.clock = TestClock::new(Sleep::Real(Duration::from_millis(10)), 0);
    let flow = GithubSignIn::new(deps);
    let f = flow.start().await.unwrap();
    assert_eq!(f.user_code, DEVICE_CODE);
    wait_until("a first poll", || server.polls() >= 1).await;
    assert!(store.token.lock().unwrap().is_none());
    server.approve();
    wait_until("the token", || store.token.lock().unwrap().is_some()).await;
    assert_eq!(store.token.lock().unwrap().as_deref(), Some(FAKE_TOKEN));
    assert_eq!(
        store.account.lock().unwrap().as_ref().map(|a| a.0.as_str()),
        Some(FAKE_LOGIN)
    );
    server.close().await;
}
