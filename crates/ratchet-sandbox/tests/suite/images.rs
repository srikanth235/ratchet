//! Port of `packages/aws/src/images.test.ts` (images lookup by tag, and the per-optimization region).

use crate::common::{Log, rec};
use aws_sdk_ec2::operation::describe_images::{DescribeImagesInput, DescribeImagesOutput};
use aws_sdk_ec2::types::{ArchitectureValues, Image, ImageState, Tag};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use ratchet_sandbox::images::{
    Ec2ImageLookup, ImageLookupError, ImageOverride, ImageSource, MockImageLookup, RatchetArch,
    RatchetImageKind, RawImage, ResolveImageArgs, image_version_matches, resolve_image_refs,
};
use ratchet_sandbox::ports::ImageLookup;
use ratchet_sandbox::region::{DEFAULT_REGION, environment_region, region_for_optimization};
use serde_json::json;

fn img(id: &str, kind: &str, version: &str, created: &str) -> Image {
    Image::builder()
        .image_id(id)
        .name(format!("ratchet-{kind}-{version}-x86_64"))
        .state(ImageState::Available)
        .architecture(ArchitectureValues::X8664)
        .creation_date(created)
        .tags(Tag::builder().key("ratchet:image").value(kind).build())
        .tags(
            Tag::builder()
                .key("ratchet:image_version")
                .value(version)
                .build(),
        )
        .build()
}

fn lookup(images: Vec<Image>, log: &Log<DescribeImagesInput>) -> Ec2ImageLookup {
    let rule = mock!(aws_sdk_ec2::Client::describe_images)
        .match_requests(rec(log))
        .then_output(move || {
            let mut b = DescribeImagesOutput::builder();
            for i in &images {
                b = b.images(i.clone());
            }
            b.build()
        });
    Ec2ImageLookup::new(mock_client!(aws_sdk_ec2, RuleMode::MatchAny, &[&rule]))
}

// ---- imageVersionMatches --------------------------------------------------------------------------------

#[test]
fn matches_the_version_itself_or_the_version_plus_a_build_suffix_never_a_longer_version() {
    assert!(image_version_matches("0.4.0", "0.4.0"));
    assert!(image_version_matches("0.4.0-20261030-1a2b3c4", "0.4.0"));
    assert!(image_version_matches("0.4.0+dev", "0.4.0"));
    assert!(!image_version_matches("0.4.01", "0.4.0"));
    assert!(!image_version_matches("0.4.0.1", "0.4.0"));
    assert!(!image_version_matches("0.4.1", "0.4.0"));
    assert!(!image_version_matches("", "0.4.0"));
}

// ---- findRatchetImages ----------------------------------------------------------------------------------

#[tokio::test]
async fn asks_ec2_for_this_accounts_available_tagged_images_of_the_architecture_and_picks_the_newest_matching_the_version()
 {
    let log = Log::new();
    let l = lookup(
        vec![
            img(
                "ami-0000000000000001",
                "runner",
                "0.4.0-20261001-aaaaaaa",
                "2026-10-01T10:00:00.000Z",
            ),
            img(
                "ami-0000000000000002",
                "runner",
                "0.4.0-20261020-bbbbbbb",
                "2026-10-20T10:00:00.000Z",
            ),
            img(
                "ami-0000000000000003",
                "runner",
                "0.3.0-20260901-ccccccc",
                "2026-09-01T10:00:00.000Z",
            ),
            img(
                "ami-0000000000000004",
                "bench",
                "0.4.0-20261020-bbbbbbb",
                "2026-10-20T10:00:00.000Z",
            ),
        ],
        &log,
    );
    let found = l.find_images("0.4.0", RatchetArch::X86_64).await.unwrap();
    assert_eq!(found.runner.as_ref().unwrap().id, "ami-0000000000000002");
    assert_eq!(found.bench.as_ref().unwrap().id, "ami-0000000000000004");
    assert_eq!(
        found.other_versions.runner,
        [
            "0.3.0-20260901-ccccccc",
            "0.4.0-20261001-aaaaaaa",
            "0.4.0-20261020-bbbbbbb"
        ]
    );

    let call = log.first();
    assert_eq!(call.owners(), ["self"]);
    let filters: Vec<_> = call
        .filters()
        .iter()
        .map(|f| (f.name().unwrap().to_string(), f.values().to_vec()))
        .collect();
    assert_eq!(
        filters,
        [
            (
                "tag:ratchet:image".to_string(),
                vec!["runner".to_string(), "bench".to_string()]
            ),
            ("state".to_string(), vec!["available".to_string()]),
            ("architecture".to_string(), vec!["x86_64".to_string()]),
        ]
    );
}

#[tokio::test]
async fn never_picks_the_host_toolchain_bench_variant_or_an_image_of_another_kind_and_honors_the_architecture()
 {
    let log = Log::new();
    let l = lookup(
        vec![
            img(
                "ami-0000000000000005",
                "bench-host",
                "0.4.0",
                "2026-10-30T10:00:00.000Z",
            ),
            img(
                "ami-0000000000000006",
                "bench",
                "0.4.0",
                "2026-10-01T10:00:00.000Z",
            ),
        ],
        &log,
    );
    let found = l.find_images("0.4.0", RatchetArch::Arm64).await.unwrap();
    assert_eq!(found.bench.unwrap().id, "ami-0000000000000006");
    assert!(
        log.first()
            .filters()
            .iter()
            .any(|f| f.name() == Some("architecture") && f.values() == ["arm64"])
    );
}

#[tokio::test]
async fn reports_nothing_for_a_version_that_was_not_built_and_lists_what_exists() {
    let log = Log::new();
    let l = lookup(
        vec![img(
            "ami-0000000000000007",
            "bench",
            "0.3.0",
            "2026-09-01T10:00:00.000Z",
        )],
        &log,
    );
    let found = l.find_images("0.4.0", RatchetArch::X86_64).await.unwrap();
    assert!(found.bench.is_none());
    assert_eq!(found.other_versions.bench, ["0.3.0"]);
}

#[tokio::test]
async fn rejects_a_version_that_could_smuggle_anything_into_a_filter() {
    let log = Log::new();
    let l = lookup(vec![], &log);
    let e = l
        .find_images("0.4.0 OR *", RatchetArch::X86_64)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not a plain version"), "{e}");
    assert!(log.is_empty());
}

// ---- resolveImageRefs -----------------------------------------------------------------------------------

fn args(
    need: Option<Vec<RatchetImageKind>>,
    region: &str,
    over: ImageOverride,
) -> ResolveImageArgs {
    ResolveImageArgs {
        app_version: "0.4.0".into(),
        region: region.into(),
        need,
        override_ids: over,
        arch: None,
    }
}

#[tokio::test]
async fn uses_the_tag_lookup_when_no_override_is_given_so_the_user_never_types_an_ami_id() {
    let l = lookup(
        vec![
            img(
                "ami-0000000000000010",
                "runner",
                "0.4.0",
                "2026-10-20T10:00:00.000Z",
            ),
            img(
                "ami-0000000000000011",
                "bench",
                "0.4.0",
                "2026-10-20T10:00:00.000Z",
            ),
        ],
        &Log::new(),
    );
    let r = resolve_image_refs(&l, &args(None, "us-east-1", ImageOverride::default()))
        .await
        .unwrap();
    assert_eq!(
        (r.runner.as_deref(), r.bench.as_deref()),
        (Some("ami-0000000000000010"), Some("ami-0000000000000011"))
    );
    assert_eq!(
        (r.source.runner, r.source.bench),
        (Some(ImageSource::Tag), Some(ImageSource::Tag))
    );
}

#[tokio::test]
async fn an_explicit_override_wins_and_is_not_looked_up_a_lookup_is_skipped_when_everything_is_overridden()
 {
    let log = Log::new();
    let l = lookup(vec![], &log);
    let over = ImageOverride {
        runner: Some("ami-0123456789abcdef0".into()),
        bench: Some("ami-0fedcba9876543210".into()),
    };
    let r = resolve_image_refs(&l, &args(None, "us-east-1", over))
        .await
        .unwrap();
    assert_eq!(
        (r.runner.as_deref(), r.bench.as_deref()),
        (Some("ami-0123456789abcdef0"), Some("ami-0fedcba9876543210"))
    );
    assert_eq!(
        (r.source.runner, r.source.bench),
        (Some(ImageSource::Override), Some(ImageSource::Override))
    );
    assert!(log.is_empty());
}

#[tokio::test]
async fn looks_up_only_what_is_not_overridden_and_only_what_is_needed() {
    let l = lookup(
        vec![img(
            "ami-0000000000000011",
            "bench",
            "0.4.0",
            "2026-10-20T10:00:00.000Z",
        )],
        &Log::new(),
    );
    let r = resolve_image_refs(
        &l,
        &args(
            Some(vec![RatchetImageKind::Bench]),
            "us-east-1",
            ImageOverride::default(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(r.bench.as_deref(), Some("ami-0000000000000011"));
    assert_eq!(r.runner, None);
    let over = ImageOverride {
        runner: Some("ami-0123456789abcdef0".into()),
        bench: None,
    };
    let mixed = resolve_image_refs(&l, &args(None, "us-east-1", over))
        .await
        .unwrap();
    assert_eq!(
        (mixed.source.runner, mixed.source.bench),
        (Some(ImageSource::Override), Some(ImageSource::Tag))
    );
}

#[tokio::test]
async fn fails_with_an_actionable_error_when_a_needed_image_is_not_published_for_this_version() {
    let l = lookup(
        vec![img(
            "ami-0000000000000007",
            "bench",
            "0.3.0",
            "2026-09-01T10:00:00.000Z",
        )],
        &Log::new(),
    );
    let e = resolve_image_refs(
        &l,
        &args(
            Some(vec![RatchetImageKind::Bench]),
            "eu-west-1",
            ImageOverride::default(),
        ),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            e,
            ratchet_sandbox::Error::ImageLookup(ImageLookupError {
                kind: RatchetImageKind::Bench,
                ..
            })
        ),
        "{e:?}"
    );
    let msg = e.to_string();
    assert!(
        msg.contains("no Ratchet bench image for version 0.4.0 (x86_64) in eu-west-1"),
        "{msg}"
    );
    assert!(msg.contains("found versions: 0.3.0"));
    assert!(msg.contains("infra/images/build.sh bench"));
    assert!(msg.contains("Advanced"));
}

#[tokio::test]
async fn says_found_none_when_nothing_of_that_kind_exists() {
    let l = lookup(vec![], &Log::new());
    let e = resolve_image_refs(
        &l,
        &args(
            Some(vec![RatchetImageKind::Runner]),
            "us-east-1",
            ImageOverride::default(),
        ),
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("; found none."), "{e}");
}

#[tokio::test]
async fn rejects_an_override_that_is_not_an_ami_id() {
    let l = lookup(vec![], &Log::new());
    let over = ImageOverride {
        runner: None,
        bench: Some("ubuntu".into()),
    };
    let e = resolve_image_refs(&l, &args(None, "us-east-1", over))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not an ami-... id"), "{e}");
}

// ---- the mock lookup ------------------------------------------------------------------------------------

#[tokio::test]
async fn the_mock_lookup_applies_the_same_selection_rules_and_records_lookups() {
    let m = MockImageLookup::with_version("0.4.0");
    let r = resolve_image_refs(&m, &args(None, "us-east-1", ImageOverride::default()))
        .await
        .unwrap();
    assert!(r.runner.is_some() && r.bench.is_some());
    assert_eq!(m.calls(), [("0.4.0".to_string(), RatchetArch::X86_64)]);
    // another version, or another architecture, finds nothing
    assert!(
        m.find_images("0.5.0", RatchetArch::X86_64)
            .await
            .unwrap()
            .runner
            .is_none()
    );
    assert!(
        m.find_images("0.4.0", RatchetArch::Arm64)
            .await
            .unwrap()
            .bench
            .is_none()
    );
    // raw images that are not available are ignored
    let raw = RawImage {
        id: Some("ami-0000000000000099".into()),
        state: Some("pending".into()),
        tags: vec![
            ("ratchet:image".into(), "bench".into()),
            ("ratchet:image_version".into(), "0.4.0".into()),
        ],
        ..Default::default()
    };
    assert!(
        MockImageLookup::new(vec![raw])
            .find_images("0.4.0", RatchetArch::X86_64)
            .await
            .unwrap()
            .bench
            .is_none()
    );
}

// ---- region per optimization (environment.region) -------------------------------------------------------

#[test]
fn reads_the_repos_region_defensively_and_falls_back_to_the_setting_then_to_us_east_1() {
    assert_eq!(
        environment_region(&json!({ "environment": { "region": "eu-west-1" } }))
            .unwrap()
            .as_deref(),
        Some("eu-west-1")
    );
    assert_eq!(environment_region(&json!({})).unwrap(), None);
    assert_eq!(
        environment_region(&json!({ "environment": "nope" })).unwrap(),
        None
    );
    assert_eq!(environment_region(&json!(null)).unwrap(), None);
    assert_eq!(
        region_for_optimization(
            &json!({ "environment": { "region": "ap-southeast-2" } }),
            Some("us-west-2")
        )
        .unwrap(),
        "ap-southeast-2"
    );
    assert_eq!(
        region_for_optimization(&json!({}), Some("us-west-2")).unwrap(),
        "us-west-2"
    );
    assert_eq!(
        region_for_optimization(&json!({}), None).unwrap(),
        DEFAULT_REGION
    );
    assert_eq!(DEFAULT_REGION, "us-east-1");
}

#[test]
fn refuses_a_value_that_is_not_a_region_name_instead_of_launching_somewhere_else() {
    for bad in [
        json!("europe"),
        json!("us_east_1"),
        json!("US-EAST-1"),
        json!("us-east-1; rm"),
        json!(7),
    ] {
        let e = environment_region(&json!({ "environment": { "region": bad } })).unwrap_err();
        assert!(
            e.to_string().contains("not an AWS region name"),
            "{bad}: {e}"
        );
    }
    let e = region_for_optimization(&json!({}), Some("not a region")).unwrap_err();
    assert!(e.to_string().contains("not an AWS region name"), "{e}");
}
