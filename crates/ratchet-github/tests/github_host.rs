//! Port of `packages/github/src/github.test.ts` and the `GitHubGitHost.commitFiles` half of `commit-files.test.ts`.
//!
//! The TS tests mocked Octokit; here the REST side is the in-process fake GitHub server (`testing::FakeGithubServer`),
//! and the git side is real git against a local bare repo with the GitHub URL swapped in by a `RecordingRunner`.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use ratchet_core::ports::{CommitFile, CommitFilesInput, FileDiff, GitHost, PullRequestInput};
use ratchet_github::testing::{FakeGithubConfig, FakeGithubServer, RecordingRunner, test_client};
use ratchet_github::{
    BaseGitHostOptions, CommandGitRunner, GitHubGitHost, GitHubGitHostOptions, GitRunOptions,
    GitRunner,
};

const TOKEN: &str = "ghp_SuperSecretToken1234567890";
const GH_URL: &str = "https://github.com/acme/search.git";

fn token_b64() -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{TOKEN}"))
}

fn s(v: &str) -> String {
    v.to_string()
}

struct Env {
    fix: Fixture,
    server: FakeGithubServer,
    runner: Arc<RecordingRunner>,
    host: GitHubGitHost,
}

/// The remote has branch `ratchet/o1/claude` with one candidate commit (`cand`); the host talks to the fake server.
async fn env(with_candidate: bool) -> (Env, Option<String>) {
    let mut fix = bare_fixture_from(&demo_dir()).await;
    let mut cand = None;
    if with_candidate {
        let w = fix.remote.work_dir.clone();
        git(&w, &["checkout", "-q", "-b", "ratchet/o1/claude"]);
        let sha = commit(&w, "src/a.txt", "one\n", "Cache\n\nRatchet-Candidate: true");
        git(&w, &["push", "-q", "origin", "ratchet/o1/claude"]);
        cand = Some(sha);
    }
    fix.shas.clear();
    let server = FakeGithubServer::start(FakeGithubConfig::default()).await;
    let runner = Arc::new(RecordingRunner::new(
        Arc::new(CommandGitRunner::new([s(TOKEN), token_b64()])),
        vec![(
            s(GH_URL),
            fix.remote.remote_dir.to_string_lossy().into_owned(),
        )],
    ));
    let host = mk_host(&fix, &server, runner.clone());
    (
        Env {
            fix,
            server,
            runner,
            host,
        },
        cand,
    )
}

fn mk_host(fix: &Fixture, server: &FakeGithubServer, runner: Arc<dyn GitRunner>) -> GitHubGitHost {
    let mut o = GitHubGitHostOptions::new("acme/search", TOKEN);
    o.cache_dir = Some(tempfile::tempdir().unwrap().keep().join("mirror.git"));
    o.api_url = Some(server.base_url.clone());
    o.http = Some(test_client());
    o.base = BaseGitHostOptions {
        runner: Some(runner),
        ..Default::default()
    };
    let _ = fix;
    GitHubGitHost::new(o).unwrap()
}

#[tokio::test]
async fn reads_candidates_creates_branches_and_opens_a_squashed_pr_via_rest() {
    let (e, cand) = env(true).await;
    let cand = cand.unwrap();
    assert_eq!(e.host.remote_url(), GH_URL);
    let found = e
        .host
        .list_candidates("ratchet/o1/claude", None)
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
        [cand.as_str()]
    );
    assert_eq!(
        e.host.changed_files("main", &cand).await.unwrap(),
        ["src/a.txt"]
    );

    e.host
        .create_branch("main", "ratchet/o1/codex")
        .await
        .unwrap();
    assert_eq!(
        e.fix
            .remote_git(&["rev-parse", "refs/heads/ratchet/o1/codex"]),
        e.fix.remote.base_sha
    );

    let pr = e
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/result"),
            title: s("ratchet: p95 100 → 80"),
            body: s("REPORT"),
            shas: vec![cand.clone()],
            commit_message: Some(s(
                "ratchet: squash\n\nCo-authored-by: Claude Code <noreply@anthropic.com>",
            )),
        })
        .await
        .unwrap();
    assert_eq!(pr.url, format!("{}/acme/search/pull/1", e.server.base_url));
    assert_eq!(
        e.fix
            .remote_git(&["rev-parse", "refs/heads/ratchet/o1/result^{tree}"]),
        e.fix
            .remote_git(&["rev-parse", &format!("{cand}^{{tree}}")])
    );
    let pulls = e.server.pulls();
    assert_eq!(pulls.len(), 1);
    assert_eq!(
        (
            pulls[0].owner.as_str(),
            pulls[0].repo.as_str(),
            pulls[0].title.as_str(),
            pulls[0].head.as_str(),
            pulls[0].base.as_str(),
            pulls[0].body.as_str()
        ),
        (
            "acme",
            "search",
            "ratchet: p95 100 → 80",
            "ratchet/o1/result",
            "main",
            "REPORT"
        )
    );
    // the PR was created with the token as a bearer credential, nothing else
    assert!(e.server.authorizations().iter().all(|t| t == TOKEN));

    // checkout works from the mirror
    let dir = e.fix.path("co");
    e.host.checkout(&cand, &dir).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("src/a.txt")).unwrap(),
        "one\n"
    );
    e.server.close().await;
}

#[tokio::test]
async fn never_puts_the_token_in_argv_it_travels_only_in_the_git_config_env_of_network_calls() {
    let (e, cand) = env(true).await;
    let cand = cand.unwrap();
    e.host
        .list_candidates("ratchet/o1/claude", None)
        .await
        .unwrap();
    e.host
        .create_branch("main", "ratchet/o1/another")
        .await
        .unwrap();
    e.host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/result3"),
            title: s("t"),
            body: s("b"),
            shas: vec![cand],
            ..Default::default()
        })
        .await
        .unwrap();

    let calls = e.runner.calls();
    assert!(calls.len() > 5, "{} calls", calls.len());
    let b64 = token_b64();
    let creds_in_url = regex::Regex::new(r"https://[^/\s]*@").unwrap();
    for c in &calls {
        let argv = c.args.join("\n");
        assert!(!argv.contains(TOKEN));
        assert!(!argv.contains(&b64));
        assert!(
            !creds_in_url.is_match(&argv),
            "no credentials in a URL: {argv}"
        );
    }
    let network: Vec<_> = calls
        .iter()
        .filter(|c| c.args.iter().any(|a| a == "fetch" || a == "push"))
        .collect();
    assert!(network.len() >= 3);
    for c in network {
        assert_eq!(c.env.get("GIT_CONFIG_COUNT").map(String::as_str), Some("1"));
        assert_eq!(
            c.env.get("GIT_CONFIG_KEY_0").map(String::as_str),
            Some("http.https://github.com/.extraheader")
        );
        assert_eq!(
            c.env.get("GIT_CONFIG_VALUE_0"),
            Some(&format!("AUTHORIZATION: basic {b64}"))
        );
    }
    e.server.close().await;
}

type Call = (Vec<String>, Vec<(String, String)>);

/// A runner that answers like git would for a diff, and records the calls.
struct ScriptedRunner {
    calls: Mutex<Vec<Call>>,
}

#[async_trait]
impl GitRunner for ScriptedRunner {
    async fn run(&self, args: &[String], opts: &GitRunOptions) -> ratchet_github::Result<String> {
        self.calls
            .lock()
            .unwrap()
            .push((args.to_vec(), opts.env.clone()));
        let (h, b, m) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        let cmd: Vec<&str> = args.iter().skip(2).map(String::as_str).collect(); // drop --git-dir <dir>
        Ok(match cmd.first().copied() {
            Some("rev-parse") => format!("{}\n", if cmd.last().unwrap().contains("main") { b } else { h }),
            Some("merge-base") => format!("{m}\n"),
            Some("diff") if cmd.contains(&"--numstat") => "3\t1\tsrc/search.ts\0-\t-\tlogo.png\0".to_string(),
            Some("diff") => "diff --git a/src/search.ts b/src/search.ts\nindex 1..2 100644\n--- a/src/search.ts\n+++ b/src/search.ts\n@@ -1,2 +1,4 @@\n-a\n+b\n+c\n+d\n x\ndiff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n".to_string(),
            _ => String::new(),
        })
    }
}

#[tokio::test]
async fn diff_syncs_the_mirror_diffs_against_the_merge_base_and_never_puts_the_token_in_argv_mocked_git()
 {
    let runner = Arc::new(ScriptedRunner {
        calls: Mutex::new(vec![]),
    });
    let server = FakeGithubServer::start(FakeGithubConfig::default()).await;
    let fix = bare_fixture_from(&demo_dir()).await;
    let host = mk_host(&fix, &server, runner.clone());
    let head = "a".repeat(40);
    let merge_base = "c".repeat(40);

    let files = host.diff("main", &head).await.unwrap();
    assert_eq!(
        files,
        vec![
            FileDiff {
                path: s("src/search.ts"),
                additions: 3,
                deletions: 1,
                binary: false,
                patch: s("@@ -1,2 +1,4 @@\n-a\n+b\n+c\n+d\n x")
            },
            FileDiff {
                path: s("logo.png"),
                additions: 0,
                deletions: 0,
                binary: true,
                patch: String::new()
            },
        ]
    );
    let calls = runner.calls.lock().unwrap().clone();
    let fetch = calls
        .iter()
        .find(|(a, _)| a.iter().any(|x| x == "fetch"))
        .unwrap();
    assert!(fetch.1.contains(&(
        s("GIT_CONFIG_VALUE_0"),
        format!("AUTHORIZATION: basic {}", token_b64())
    )));
    let diffs: Vec<_> = calls
        .iter()
        .filter(|(a, _)| a.iter().any(|x| x == "diff"))
        .collect();
    assert_eq!(
        diffs
            .iter()
            .map(|(a, _)| a[a.len() - 2..].to_vec())
            .collect::<Vec<_>>(),
        vec![vec![merge_base.clone(), head.clone()]; 2]
    );
    for (a, _) in calls.iter() {
        assert!(!a.join(" ").contains(TOKEN));
    }
    server.close().await;
}

#[tokio::test]
async fn scrubs_the_token_from_git_errors() {
    // default runner against an unreachable host
    let mut o = GitHubGitHostOptions::new("acme/search", TOKEN);
    o.host = Some(s("localhost.invalid"));
    o.cache_dir = Some(tempfile::tempdir().unwrap().keep().join("dead.git"));
    let host = GitHubGitHost::new(o).unwrap();
    let err = host.list_candidates("main", None).await.unwrap_err();
    assert!(!err.to_string().contains(TOKEN), "{err}");
    assert!(!err.to_string().contains(&token_b64()), "{err}");
}

#[tokio::test]
async fn the_git_runner_redacts_secrets_from_failure_messages() {
    let run = CommandGitRunner::new([s(TOKEN)]);
    let err = run
        .run(&[format!("--bogus-{TOKEN}")], &GitRunOptions::default())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("***"), "{err}");
    assert!(!err.to_string().contains(TOKEN), "{err}");
}

#[tokio::test]
async fn refreshes_the_existing_pr_when_github_answers_422() {
    let (e, _) = env(false).await;
    let first = e
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/claude"),
            title: s("Old"),
            body: s("old body"),
            ..Default::default()
        })
        .await
        .unwrap();
    // the same head again: the fake answers 422 (a PR already exists), the host finds it and updates it
    let again = e
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/claude"),
            title: s("T"),
            body: s("B"),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(again.url, first.url);
    let pulls = e.server.pulls();
    assert_eq!(pulls.len(), 1);
    assert_eq!(
        (pulls[0].title.as_str(), pulls[0].body.as_str()),
        ("T", "B")
    );
    let requests = e.server.requests();
    assert!(requests.contains(&s("GET /repos/acme/search/pulls")));
    assert!(requests.contains(&s("PATCH /repos/acme/search/pulls/1")));
    e.server.close().await;
}

#[tokio::test]
async fn rethrows_a_422_when_no_open_pr_matches() {
    let (e, _) = env(false).await;
    e.server.fail_next_pull_create(422, "Validation Failed");
    let err = e
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/claude"),
            title: s("T"),
            body: s("B"),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(422));
    assert!(err.to_string().contains("Validation Failed"));
    e.server.close().await;
}

#[tokio::test]
async fn rethrows_non_422_rest_errors() {
    let (e, _) = env(false).await;
    e.server.fail_next_pull_create(403, "Forbidden");
    let err = e
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/claude"),
            title: s("T"),
            body: s("B"),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(err.status(), Some(403));
    assert!(err.to_string().contains("Forbidden"));
    e.server.close().await;
}

#[test]
fn validates_repo_and_token() {
    assert!(
        GitHubGitHost::new(GitHubGitHostOptions::new("nope", TOKEN))
            .err()
            .unwrap()
            .to_string()
            .contains("owner/name")
    );
    assert!(
        GitHubGitHost::new(GitHubGitHostOptions::new("a/b", ""))
            .err()
            .unwrap()
            .to_string()
            .contains("token")
    );
}

// ------------------------------------------------------------------ commit-files.test.ts: GitHubGitHost.commitFiles

fn template_files() -> Vec<CommitFile> {
    vec![
        CommitFile::new("ratchet.yaml", "version: 0\n"),
        CommitFile::new("RATCHET.md", "# brief\n"),
        CommitFile::new("bench/run.mjs", "console.log(\"{}\")\n").executable(),
        CommitFile::new("src/new/deep/file.txt", "x"),
    ]
}

#[tokio::test]
async fn commit_files_pushes_the_branch_and_opens_the_pr_from_it_no_squash_head_already_exists() {
    let (e, _) = env(false).await;
    let sha = e
        .host
        .commit_files(&CommitFilesInput {
            base_ref: s("main"),
            branch: s("ratchet/template-gh"),
            message: s("Add template"),
            files: template_files(),
            update: false,
        })
        .await
        .unwrap()
        .sha;
    assert_eq!(
        e.fix
            .remote_git(&["rev-parse", "refs/heads/ratchet/template-gh"]),
        sha
    );
    assert_eq!(
        e.fix.remote_git(&["rev-parse", &format!("{sha}^")]),
        e.fix.remote.base_sha
    );

    let pr = e
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/template-gh"),
            title: s("Add template"),
            body: s("body"),
            shas: vec![],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(pr.url, format!("{}/acme/search/pull/1", e.server.base_url));
    let pulls = e.server.pulls();
    assert_eq!(
        (
            pulls[0].head.as_str(),
            pulls[0].base.as_str(),
            pulls[0].body.as_str()
        ),
        ("ratchet/template-gh", "main", "body")
    );
    assert_eq!(
        e.fix
            .remote_git(&["rev-parse", "refs/heads/ratchet/template-gh"]),
        sha
    );

    // the token only travels in the environment of the network calls, never in argv
    let calls = e.runner.calls();
    assert!(
        !calls
            .iter()
            .flat_map(|c| c.args.iter())
            .any(|a| a.contains(TOKEN))
    );
    assert!(calls.iter().any(|c| c.args.iter().any(|a| a == "push")
        && c.env.values().any(|v| v.contains("AUTHORIZATION"))));
    e.server.close().await;
}

#[tokio::test]
async fn commit_files_refuses_a_branch_that_already_exists_on_the_remote() {
    let (e, _) = env(false).await;
    e.host
        .commit_files(&CommitFilesInput {
            base_ref: s("main"),
            branch: s("ratchet/template-gh"),
            message: s("Add template"),
            files: template_files(),
            update: false,
        })
        .await
        .unwrap();
    // a second host with a fresh mirror
    let second = mk_host(&e.fix, &e.server, e.runner.clone());
    let again = second
        .commit_files(&CommitFilesInput {
            base_ref: s("main"),
            branch: s("ratchet/template-gh"),
            message: s("again"),
            files: template_files(),
            update: false,
        })
        .await;
    assert!(again.is_err());
    e.server.close().await;
}
