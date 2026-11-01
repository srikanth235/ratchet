//! Port of `packages/github/src/access.test.ts` (token validation), against the fake GitHub server.

use ratchet_github::testing::{
    FakeGithubConfig, FakeGithubServer, repo_with_permissions, test_client,
};
use ratchet_github::{CheckGitHubAccessOptions, GitHubAccess, check_github_access};

const TOKEN: &str = "ghp_SuperSecretToken1234567890";

fn opts(repo: &str, token: &str, server: &FakeGithubServer) -> CheckGitHubAccessOptions {
    let mut o = CheckGitHubAccessOptions::new(repo, token);
    o.api_url = Some(server.base_url.clone());
    o.http = Some(test_client());
    o
}

async fn server_with(repos: Vec<serde_json::Value>) -> FakeGithubServer {
    FakeGithubServer::start(FakeGithubConfig {
        repos,
        ..Default::default()
    })
    .await
}

#[tokio::test]
async fn reports_the_repo_default_branch_and_whether_the_token_can_push() {
    let rw = server_with(vec![repo_with_permissions(
        "acme/search",
        "main",
        true,
        true,
    )])
    .await;
    let got = check_github_access(opts("acme/search", TOKEN, &rw))
        .await
        .unwrap();
    assert_eq!(
        got,
        GitHubAccess {
            full_name: "acme/search".into(),
            default_branch: "main".into(),
            private: true,
            can_push: true
        }
    );
    assert!(
        rw.requests()
            .contains(&"GET /repos/acme/search".to_string())
    );
    assert_eq!(rw.authorizations(), [TOKEN]);

    let ro = server_with(vec![repo_with_permissions(
        "acme/search",
        "main",
        false,
        false,
    )])
    .await;
    assert!(
        !check_github_access(opts("acme/search.git", TOKEN, &ro))
            .await
            .unwrap()
            .can_push
    );
}

#[tokio::test]
async fn gives_actionable_errors_and_never_echoes_the_token() {
    let server = server_with(vec![repo_with_permissions("a/b", "main", false, true)]).await;

    server.fail_next_request(401, "Bad credentials");
    let e = check_github_access(opts("a/b", TOKEN, &server))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("rejected the token"), "{e}");

    let e = check_github_access(opts("a/missing", TOKEN, &server))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not found"), "{e}");

    server.fail_next_request(500, &format!("boom {TOKEN}"));
    let e = check_github_access(opts("a/b", TOKEN, &server))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("(500)"), "{e}");
    assert!(!e.to_string().contains(TOKEN), "{e}");
}

#[tokio::test]
async fn validates_its_inputs_before_any_request() {
    let server = server_with(vec![]).await;
    let e = check_github_access(opts("nonsense", TOKEN, &server))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("owner/name"), "{e}");
    let e = check_github_access(opts("a/b", "", &server))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("token is required"), "{e}");
    assert!(server.requests().is_empty());
}
