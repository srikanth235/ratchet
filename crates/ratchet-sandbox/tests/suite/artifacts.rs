use crate::common::{Log, meta, rec, rec_with};
use aws_sdk_s3::operation::get_object::{GetObjectError, GetObjectInput, GetObjectOutput};
use aws_sdk_s3::operation::list_objects_v2::{ListObjectsV2Input, ListObjectsV2Output};
use aws_sdk_s3::operation::put_object::{PutObjectInput, PutObjectOutput};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Object, ServerSideEncryption};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use ratchet_core::ports::ArtifactStore;
use ratchet_sandbox::artifacts::{FsArtifactStore, S3ArtifactStore};

fn store(client: aws_sdk_s3::Client) -> S3ArtifactStore {
    S3ArtifactStore::new(client, "b").with_prefix("artifacts/")
}

fn object(key: &str) -> Object {
    Object::builder().key(key).build()
}

// ---- S3ArtifactStore (artifacts.test.ts) ------------------------------------------------------------

#[tokio::test]
async fn s3_put_encrypts_and_prefixes() {
    type Put = (
        Option<String>,
        Option<String>,
        Option<ServerSideEncryption>,
        Vec<u8>,
    );
    let puts: Log<Put> = Log::new();
    let rule = mock!(aws_sdk_s3::Client::put_object)
        .match_requests(rec_with(&puts, |i: &PutObjectInput| {
            (
                i.bucket().map(str::to_string),
                i.key().map(str::to_string),
                i.server_side_encryption().cloned(),
                i.body.bytes().unwrap_or_default().to_vec(),
            )
        }))
        .then_output(|| PutObjectOutput::builder().build());
    let client = mock_client!(aws_sdk_s3, RuleMode::MatchAny, &[&rule]);
    store(client)
        .put_str("o1/a1/events.jsonl", "x")
        .await
        .unwrap();
    assert_eq!(
        puts.first(),
        (
            Some("b".into()),
            Some("artifacts/o1/a1/events.jsonl".into()),
            Some(ServerSideEncryption::Aes256),
            b"x".to_vec()
        )
    );
}

#[tokio::test]
async fn s3_get_returns_body_or_none_for_no_such_key() {
    let gets: Log<GetObjectInput> = Log::new();
    let ok = mock!(aws_sdk_s3::Client::get_object)
        .match_requests({
            let gets = gets.clone();
            move |i: &GetObjectInput| {
                gets.push(i.clone());
                i.key() == Some("artifacts/k")
            }
        })
        .then_output(|| {
            GetObjectOutput::builder()
                .body(ByteStream::from_static(b"hello"))
                .build()
        });
    let missing = mock!(aws_sdk_s3::Client::get_object)
        .match_requests(|i: &GetObjectInput| i.key() == Some("artifacts/missing"))
        .then_error(|| GetObjectError::generic(meta("NoSuchKey", "nf")));
    let denied = mock!(aws_sdk_s3::Client::get_object)
        .match_requests(|i: &GetObjectInput| i.key() == Some("artifacts/x"))
        .then_error(|| GetObjectError::generic(meta("AccessDenied", "denied")));
    let s = store(mock_client!(
        aws_sdk_s3,
        RuleMode::MatchAny,
        &[&ok, &missing, &denied]
    ));
    assert_eq!(s.get("k").await.unwrap().as_deref(), Some("hello"));
    assert_eq!(gets.first().bucket(), Some("b"));
    assert_eq!(gets.first().key(), Some("artifacts/k"));
    assert_eq!(s.get("missing").await.unwrap(), None);
    let err = s.get("x").await.unwrap_err();
    assert!(err.to_string().contains("denied"), "{err}");
}

#[tokio::test]
async fn s3_get_treats_the_typed_no_such_key_error_as_missing_too() {
    let missing = mock!(aws_sdk_s3::Client::get_object).then_error(|| {
        GetObjectError::NoSuchKey(aws_sdk_s3::types::error::NoSuchKey::builder().build())
    });
    let s = store(mock_client!(aws_sdk_s3, RuleMode::MatchAny, &[&missing]));
    assert_eq!(s.get("gone").await.unwrap(), None);
}

#[tokio::test]
async fn s3_list_pages_and_strips_the_prefix() {
    let calls: Log<ListObjectsV2Input> = Log::new();
    let rule = mock!(aws_sdk_s3::Client::list_objects_v2)
        .match_requests(rec(&calls))
        .sequence()
        .output(|| {
            ListObjectsV2Output::builder()
                .contents(object("artifacts/p/b"))
                .contents(object("artifacts/p/a"))
                .is_truncated(true)
                .next_continuation_token("t")
                .build()
        })
        .output(|| {
            ListObjectsV2Output::builder()
                .contents(object("artifacts/p/c"))
                .build()
        })
        .build();
    let s = store(mock_client!(aws_sdk_s3, RuleMode::MatchAny, &[&rule]));
    assert_eq!(s.list("p/").await.unwrap(), ["p/a", "p/b", "p/c"]);
    let calls = calls.all();
    assert_eq!(calls[0].bucket(), Some("b"));
    assert_eq!(calls[0].prefix(), Some("artifacts/p/"));
    assert_eq!(calls[1].continuation_token(), Some("t"));
}

#[tokio::test]
async fn s3_rejects_bad_keys_before_any_request() {
    // No rules: any request would panic the mock client.
    let s = store(mock_client!(
        aws_sdk_s3,
        RuleMode::MatchAny,
        &[] as &[&aws_smithy_mocks::Rule]
    ));
    for k in ["/abs", "../x", "a/../b", ""] {
        let e = s.put_str(k, "x").await.unwrap_err();
        assert!(e.to_string().contains("invalid artifact key"), "{k}: {e}");
        assert!(s.get(k).await.is_err(), "{k}");
    }
}

// ---- FsArtifactStore --------------------------------------------------------------------------------

#[tokio::test]
async fn fs_round_trips_lists_by_prefix_nested_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let s = FsArtifactStore::new(dir.path());
    s.put_str("o1/a1/events.jsonl", "one").await.unwrap();
    s.put("o1/a2/events.jsonl", "two".as_bytes()).await.unwrap();
    s.put_str("o2/a1/x", "3").await.unwrap();
    assert_eq!(
        s.get("o1/a1/events.jsonl").await.unwrap().as_deref(),
        Some("one")
    );
    assert_eq!(
        s.get("o1/a2/events.jsonl").await.unwrap().as_deref(),
        Some("two")
    );
    assert_eq!(s.get("nope").await.unwrap(), None);
    assert_eq!(s.get("o1").await.unwrap(), None); // directory
    assert_eq!(
        s.list("o1/").await.unwrap(),
        ["o1/a1/events.jsonl", "o1/a2/events.jsonl"]
    );
    assert_eq!(s.list("zzz").await.unwrap(), Vec::<String>::new());
    s.put_str("o1/a1/events.jsonl", "overwritten")
        .await
        .unwrap();
    assert_eq!(
        s.get("o1/a1/events.jsonl").await.unwrap().as_deref(),
        Some("overwritten")
    );
}

#[tokio::test]
async fn fs_writes_through_a_staging_file_that_never_shows_up_as_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let s = FsArtifactStore::new(dir.path());
    let puts = (1..=20).map(|i| {
        let s = s.clone();
        async move {
            s.put_str(&format!("events/{i:08}.jsonl"), &format!("batch {i}\n"))
                .await
        }
    });
    for r in futures::future::join_all(puts).await {
        r.unwrap();
    }
    assert_eq!(s.list("").await.unwrap().len(), 20);
    // every staging file was renamed into place
    let staged: Vec<_> = std::fs::read_dir(dir.path().join(".put-tmp"))
        .unwrap()
        .collect();
    assert!(staged.is_empty(), "{staged:?}");
}

#[tokio::test]
async fn fs_refuses_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let s = FsArtifactStore::new(dir.path());
    let e = s.put_str("../escape", "x").await.unwrap_err();
    assert!(e.to_string().contains("invalid artifact key"), "{e}");
    assert!(s.get("a/../../escape").await.is_err());
    assert!(!dir.path().parent().unwrap().join("escape").exists());
}

#[tokio::test]
async fn fs_never_lists_the_staging_dir_and_a_failed_write_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let s = FsArtifactStore::new(dir.path());
    s.put_str("a", "1").await.unwrap();
    // `a` is a file, so `a/b` cannot be created: the put fails and no half-written object appears.
    assert!(s.put_str("a/b", "2").await.is_err());
    assert_eq!(s.list("").await.unwrap(), ["a"]);
    assert_eq!(s.get("a").await.unwrap().as_deref(), Some("1"));
}

#[tokio::test]
async fn fs_readers_see_the_old_or_the_new_object_never_a_partial_one() {
    let dir = tempfile::tempdir().unwrap();
    let s = FsArtifactStore::new(dir.path());
    let old = "o".repeat(256 * 1024);
    let new = "n".repeat(256 * 1024);
    s.put_str("k", &old).await.unwrap();
    let writer = {
        let s = s.clone();
        let new = new.clone();
        tokio::spawn(async move {
            for _ in 0..50 {
                s.put_str("k", &new).await.unwrap();
            }
        })
    };
    for _ in 0..200 {
        let v = s.get("k").await.unwrap().unwrap();
        assert!(v == old || v == new, "partial object of {} bytes", v.len());
    }
    writer.await.unwrap();
}

#[tokio::test]
async fn fs_store_behaves_like_s3_for_put_get_list_by_prefix_under_the_runner_layout() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsArtifactStore::new(dir.path());
    store
        .put("artifacts/o/a/events/00000001.jsonl", b"x")
        .await
        .unwrap();
    store
        .put("artifacts/o/a/inbox/00000001.json", b"y")
        .await
        .unwrap();
    assert_eq!(
        store
            .get("artifacts/o/a/events/00000001.jsonl")
            .await
            .unwrap()
            .as_deref(),
        Some("x")
    );
    assert_eq!(store.get("nope").await.unwrap(), None);
    assert_eq!(
        store.list("artifacts/o/a/events/").await.unwrap(),
        ["artifacts/o/a/events/00000001.jsonl"]
    );
    assert_eq!(store.list("artifacts/").await.unwrap().len(), 2);
}
