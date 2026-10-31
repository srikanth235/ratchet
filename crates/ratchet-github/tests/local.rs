//! Port of `packages/github/src/local.test.ts` and the `LocalGitHost.commitFiles` half of `commit-files.test.ts`.

mod common;

use common::*;
use ratchet_core::enums::{AgentKind, Direction};
use ratchet_core::ports::{CommitFile, CommitFilesInput, FileDiff, GitHost, PullRequestInput};
use ratchet_github::{SquashPlanInput, SquashWin, build_squash_plan};

fn s(v: &str) -> String {
    v.to_string()
}

// ------------------------------------------------------------------ local.test.ts

#[tokio::test]
async fn lists_only_commits_with_a_real_candidate_trailer_oldest_first() {
    let f = candidate_fixture().await;
    let found = f
        .host
        .list_candidates("ratchet/o1/claude", None)
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
        [f.sha("c1"), f.sha("c2")]
    );
    assert_eq!(found[0].branch, "ratchet/o1/claude");
    assert!(found[0].message.contains("Cache tokenizer"));
    assert!(found[0].committed_at > 1_600_000_000_000);
}

#[tokio::test]
async fn respects_since_sha() {
    let f = candidate_fixture().await;
    let found = f
        .host
        .list_candidates("ratchet/o1/claude", Some(f.sha("c1")))
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|c| c.sha.as_str()).collect::<Vec<_>>(),
        [f.sha("c2")]
    );
    assert!(
        f.host
            .list_candidates("ratchet/o1/claude", Some(f.sha("c2")))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn create_branch_branches_from_a_ref_and_refuses_to_overwrite() {
    let f = candidate_fixture().await;
    f.host
        .create_branch("main", "ratchet/o1/codex")
        .await
        .unwrap();
    assert!(
        f.host
            .list_candidates("ratchet/o1/codex", None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.remote_git(&["rev-parse", "refs/heads/ratchet/o1/codex"]),
        f.remote.base_sha
    );
    assert!(
        f.host
            .create_branch(f.sha("c1"), "ratchet/o1/codex")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn changed_files_lists_paths_changed_relative_to_base() {
    let f = candidate_fixture().await;
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    assert_eq!(
        sorted(f.host.changed_files("main", f.sha("c1")).await.unwrap()),
        ["src/a.txt"]
    );
    assert_eq!(
        sorted(f.host.changed_files("main", f.sha("c2")).await.unwrap()),
        ["notes.txt", "src/a.txt", "src/m.txt", "src/n.txt"]
    );
    assert!(
        f.host
            .changed_files(f.sha("c1"), f.sha("c2"))
            .await
            .unwrap()
            .contains(&s("notes.txt"))
    );
}

#[tokio::test]
async fn checkout_materializes_a_sha_without_git_dir_or_repo_side_effects() {
    let f = candidate_fixture().await;
    let dir = f.path("co");
    f.host.checkout(f.sha("c1"), &dir).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("src/a.txt")).unwrap(),
        "one\n"
    );
    assert!(dir.join("ratchet.yaml").exists());
    assert!(!dir.join(".git").exists());
    assert!(!dir.join("notes.txt").exists());
    let dir2 = f.path("co2");
    f.host.checkout(f.sha("c2"), &dir2).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir2.join("src/a.txt")).unwrap(),
        "two\n"
    );
    // Only the main worktree: the temp path itself may contain "co" by chance.
    assert_eq!(f.remote_git(&["worktree", "list"]).lines().count(), 1);
}

#[tokio::test]
async fn open_pull_request_squashes_to_the_final_verified_tree_and_writes_the_body_to_a_file() {
    let f = candidate_fixture().await;
    // c2 was authored on top of c1 (rebased), so its tree contains both wins
    let plan = build_squash_plan(&SquashPlanInput {
        metric: s("p95"),
        direction: Direction::Minimize,
        wins: vec![
            SquashWin {
                sha: s(f.sha("c1")),
                agent_kind: AgentKind::ClaudeCode,
                message: s("Cache tokenizer"),
                from_value: 100.0,
                to_value: 80.0,
            },
            SquashWin {
                sha: s(f.sha("c2")),
                agent_kind: AgentKind::Codex,
                message: s("Index postings"),
                from_value: 80.0,
                to_value: 60.0,
            },
        ],
    })
    .unwrap();
    let pr = f
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/result"),
            title: plan.title.clone(),
            body: s("# report body"),
            shas: plan.shas.clone(),
            commit_message: Some(plan.message.clone()),
        })
        .await
        .unwrap();

    let tree = |r: &str| f.remote_git(&["rev-parse", &format!("{r}^{{tree}}")]);
    assert_eq!(tree("refs/heads/ratchet/o1/result"), tree(f.sha("c2")));
    assert_eq!(
        f.remote_git(&["rev-list", "--count", "main..ratchet/o1/result"]),
        "1"
    );
    assert_eq!(
        f.remote_git(&["rev-parse", "ratchet/o1/result^"]),
        f.remote.base_sha
    );
    let msg = f.remote_git(&["log", "-1", "--format=%B", "ratchet/o1/result"]);
    assert!(msg.contains("Co-authored-by: Claude Code <noreply@anthropic.com>"));
    assert!(msg.contains("Co-authored-by: Codex <noreply@openai.com>"));

    assert!(pr.url.starts_with("file://"));
    let body_path = url::Url::parse(&pr.url).unwrap().to_file_path().unwrap();
    assert!(
        std::fs::read_to_string(body_path)
            .unwrap()
            .contains("# report body")
    );
}

#[tokio::test]
async fn squash_is_parented_at_the_merge_base_when_main_has_moved_on() {
    let f = candidate_fixture().await;
    let other = f.path("other");
    git(
        f.scratch.path(),
        &[
            "clone",
            "-q",
            f.remote.remote_dir.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    commit(&other, "later.txt", "later\n", "main moves");
    git(&other, &["push", "-q", "origin", "HEAD:main"]);

    f.host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/o1/result2"),
            title: s("t"),
            body: s("b"),
            shas: vec![s(f.sha("c2"))],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        f.remote_git(&["rev-parse", "ratchet/o1/result2^"]),
        f.remote.base_sha
    );
    assert_eq!(
        f.remote_git(&["rev-parse", "ratchet/o1/result2^{tree}"]),
        f.remote_git(&["rev-parse", &format!("{}^{{tree}}", f.sha("c2"))])
    );
}

#[tokio::test]
async fn diff_per_file_counts_and_hunks_of_a_candidate_against_its_merge_base_with_main() {
    let f = candidate_fixture().await;
    // main moves on (later.txt): that change must not show up in the candidate's diff
    let other = f.path("other");
    git(
        f.scratch.path(),
        &[
            "clone",
            "-q",
            f.remote.remote_dir.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    commit(&other, "later.txt", "later\n", "main moves");
    git(&other, &["push", "-q", "origin", "HEAD:main"]);

    let files = f.host.diff("main", f.sha("c2")).await.unwrap();
    assert_eq!(
        files.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(),
        ["notes.txt", "src/a.txt", "src/m.txt", "src/n.txt"]
    );
    let a = files.iter().find(|x| x.path == "src/a.txt").unwrap();
    assert_eq!((a.additions, a.deletions, a.binary), (1, 0, false));
    assert!(a.patch.starts_with("@@ -0,0 +1 @@"), "{}", a.patch);
    assert!(a.patch.contains("+two"));
    assert!(files.iter().all(|x| !x.patch.contains("diff --git")));
}

#[tokio::test]
async fn diff_binary_files_have_no_counts_or_patch_and_an_unchanged_tree_has_no_files() {
    let f = candidate_fixture().await;
    std::fs::write(f.remote.work_dir.join("blob.bin"), [0u8, 1, 2, 0, 255]).unwrap();
    git(&f.remote.work_dir, &["add", "-A"]);
    git(&f.remote.work_dir, &["commit", "-q", "-m", "binary"]);
    git(
        &f.remote.work_dir,
        &["push", "-q", "origin", "ratchet/o1/claude"],
    );
    let sha = git(&f.remote.work_dir, &["rev-parse", "HEAD"]);
    let files = f.host.diff("main", &sha).await.unwrap();
    let bin = files.into_iter().find(|x| x.path == "blob.bin").unwrap();
    assert_eq!(
        bin,
        FileDiff {
            path: s("blob.bin"),
            additions: 0,
            deletions: 0,
            binary: true,
            patch: String::new()
        }
    );
    assert!(f.host.diff("main", "main").await.unwrap().is_empty());
}

#[tokio::test]
async fn remote_url_is_a_file_url() {
    let f = candidate_fixture().await;
    let url = f.host.remote_url();
    assert!(
        url.starts_with("file://") && url.ends_with("remote.git"),
        "{url}"
    );
}

// ------------------------------------------------------------------ commit-files.test.ts: LocalGitHost.commitFiles

async fn commit_files_fixture() -> Fixture {
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("src")).unwrap();
    std::fs::write(src.path().join("src/app.js"), "old\n").unwrap();
    std::fs::write(src.path().join("RATCHET.md"), "the old brief\n").unwrap();
    std::fs::write(src.path().join("keep.txt"), "keep\n").unwrap();
    bare_fixture_from(src.path()).await
}

fn template_files() -> Vec<CommitFile> {
    vec![
        CommitFile::new("ratchet.yaml", "version: 0\n"),
        CommitFile::new("RATCHET.md", "# brief\n"),
        CommitFile::new("bench/run.mjs", "console.log(\"{}\")\n").executable(),
        CommitFile::new("src/new/deep/file.txt", "x"),
    ]
}

fn input(branch: &str, base: &str, files: Vec<CommitFile>) -> CommitFilesInput {
    CommitFilesInput {
        base_ref: s(base),
        branch: s(branch),
        message: s("m"),
        files,
    }
}

#[tokio::test]
async fn commit_files_creates_a_branch_with_one_commit_on_the_base_that_adds_and_replaces_files() {
    let f = commit_files_fixture().await;
    let sha = f
        .host
        .commit_files(&CommitFilesInput {
            base_ref: s("main"),
            branch: s("ratchet/template-x"),
            message: s("Add template\n\nbody"),
            files: template_files(),
        })
        .await
        .unwrap()
        .sha;
    let show = |path: &str, r: &str| f.remote_git(&["show", &format!("{r}:{path}")]);

    assert_eq!(
        f.remote_git(&["rev-parse", "refs/heads/ratchet/template-x"]),
        sha
    );
    assert_eq!(
        f.remote_git(&["rev-parse", &format!("{sha}^")]),
        f.remote.base_sha
    ); // exactly one commit on the base
    assert_eq!(
        f.remote_git(&["log", "-1", "--format=%B", &sha]),
        "Add template\n\nbody"
    );
    assert_eq!(
        f.remote_git(&["log", "-1", "--format=%an <%ae>", &sha]),
        "Ratchet <ratchet@users.noreply.github.com>"
    );
    assert_eq!(show("ratchet.yaml", &sha), "version: 0");
    assert_eq!(show("RATCHET.md", &sha), "# brief"); // replaced
    assert_eq!(show("src/new/deep/file.txt", &sha), "x");
    assert_eq!(show("keep.txt", &sha), "keep"); // the rest of the base tree is untouched
    assert_eq!(show("src/app.js", &sha), "old");
    assert!(
        f.remote_git(&["ls-tree", &sha, "bench/run.mjs"])
            .starts_with("100755")
    );
    assert!(
        f.remote_git(&["ls-tree", &sha, "ratchet.yaml"])
            .starts_with("100644")
    );
    // main is untouched, and the diff is what the PR will show
    assert_eq!(f.remote_git(&["rev-parse", "main"]), f.remote.base_sha);
    let mut changed = f.host.changed_files("main", &sha).await.unwrap();
    changed.sort();
    assert_eq!(
        changed,
        [
            "RATCHET.md",
            "bench/run.mjs",
            "ratchet.yaml",
            "src/new/deep/file.txt"
        ]
    );
}

#[tokio::test]
async fn commit_files_refuses_an_existing_branch_unsafe_or_duplicate_paths_and_an_empty_list_and_leaves_the_remote_alone()
 {
    let f = commit_files_fixture().await;
    f.host
        .commit_files(&input("ratchet/template-x", "main", template_files()))
        .await
        .unwrap();
    let before = f.remote_git(&["for-each-ref"]);

    assert!(
        f.host
            .commit_files(&input("ratchet/template-x", "main", template_files()))
            .await
            .is_err()
    );
    for path in [
        "../evil",
        "/abs",
        "a/../b",
        ".git/config",
        "a/.git/hooks/x",
        "a//b",
        "./a",
        "a\\b",
        "",
    ] {
        let err = f
            .host
            .commit_files(&input(
                "ratchet/bad",
                "main",
                vec![CommitFile::new(path, "x")],
            ))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unsafe path"), "{path:?}: {err}");
    }
    let dup = f
        .host
        .commit_files(&input(
            "ratchet/bad",
            "main",
            vec![CommitFile::new("a", "1"), CommitFile::new("a", "2")],
        ))
        .await
        .unwrap_err();
    assert!(dup.to_string().contains("duplicate"), "{dup}");
    let empty = f
        .host
        .commit_files(&input("ratchet/bad", "main", vec![]))
        .await
        .unwrap_err();
    assert!(empty.to_string().contains("no files"), "{empty}");
    let nope = f
        .host
        .commit_files(&input("ratchet/bad", "nope", template_files()))
        .await
        .unwrap_err();
    assert!(nope.to_string().contains("cannot resolve"), "{nope}");
    assert_eq!(f.remote_git(&["for-each-ref"]), before);
}

#[tokio::test]
async fn commit_files_branch_is_a_normal_pr_head_and_open_pull_request_with_no_shas_opens_it_as_is()
{
    let f = commit_files_fixture().await;
    let sha = f
        .host
        .commit_files(&input(
            "ratchet/template-y",
            "main",
            vec![CommitFile::new("y.txt", "y\n")],
        ))
        .await
        .unwrap()
        .sha;
    let pr = f
        .host
        .open_pull_request(&PullRequestInput {
            base: s("main"),
            head: s("ratchet/template-y"),
            title: s("Add y"),
            body: s("what and how"),
            shas: vec![],
            ..Default::default()
        })
        .await
        .unwrap();
    let body_path = url::Url::parse(&pr.url).unwrap().to_file_path().unwrap();
    assert!(
        std::fs::read_to_string(body_path)
            .unwrap()
            .contains("what and how")
    );
    assert_eq!(
        f.remote_git(&["rev-parse", "refs/heads/ratchet/template-y"]),
        sha
    ); // not squashed again
}

#[tokio::test]
async fn commit_files_can_base_the_commit_on_a_sha_and_keeps_multi_line_content_verbatim() {
    let f = commit_files_fixture().await;
    let content = "line1\r\nline2\n\n  indented \ttab\nunicode: é ✓\n";
    let base = f.remote.base_sha.clone();
    let sha = f
        .host
        .commit_files(&input(
            "ratchet/template-z",
            &base,
            vec![CommitFile::new("z.txt", content)],
        ))
        .await
        .unwrap()
        .sha;
    assert_eq!(
        git_raw(&f.remote.remote_dir, &["show", &format!("{sha}:z.txt")]),
        content
    );
}
