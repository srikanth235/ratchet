//! Finds the machine images (AMIs) the app launches, by TAG, so nobody types an AMI id. (TS `images.ts`)
//!
//! `infra/images` (Packer) tags every AMI and snapshot it builds with
//!
//! ```text
//! ratchet:image          runner | bench      (the legacy host-toolchain bench variant is `bench-host`: never found here)
//! ratchet:image_version  <app version>[-<build suffix>]
//! ratchet:arch           x86_64 | arm64
//! ```
//!
//! and the app asks EC2 (DescribeImages, own account only) for the newest image of each kind whose version matches the
//! app version. An explicit AMI id (the "Advanced" override, for development) always wins over the lookup.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::{LazyLock, Mutex};

use async_trait::async_trait;
use aws_sdk_ec2::types::{Filter, Image, ImageState};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, sdk};
use crate::ports::ImageLookup;
use crate::util::json_str;

pub const TAG_IMAGE: &str = "ratchet:image";
pub const TAG_IMAGE_VERSION: &str = "ratchet:image_version";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RatchetImageKind {
    Runner,
    Bench,
}

impl RatchetImageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runner => "runner",
            Self::Bench => "bench",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RatchetArch {
    #[default]
    #[serde(rename = "x86_64")]
    X86_64,
    #[serde(rename = "arm64")]
    Arm64,
}

impl RatchetArch {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Arm64 => "arm64",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatchetImage {
    pub id: String,
    pub name: String,
    /// The `ratchet:image_version` tag.
    pub version: String,
    pub arch: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OtherVersions {
    pub runner: Vec<String>,
    pub bench: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundImages {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<RatchetImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bench: Option<RatchetImage>,
    /// Versions that exist per kind but did not match, for error messages.
    pub other_versions: OtherVersions,
}

static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,39}$").expect("static regex"));
static AMI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ami-[0-9a-f]{8,17}$").expect("static regex"));

/// `tag` matches the app version when it is that version, or the version followed by a build suffix (`0.4.0` matches
/// `0.4.0`, `0.4.0-20261030-1a2b3c4`, `0.4.0+dev`), never a longer version (`0.4.0` does not match `0.4.01` or `0.4.0.1`).
pub fn image_version_matches(tag: &str, app_version: &str) -> bool {
    tag == app_version
        || tag
            .strip_prefix(app_version)
            .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('+'))
}

/// An image as the lookup sees it, independent of the SDK type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawImage {
    pub id: Option<String>,
    pub name: Option<String>,
    /// `None` counts as `available`.
    pub state: Option<String>,
    pub architecture: Option<String>,
    /// ISO-8601, so timestamps sort lexicographically.
    pub creation_date: Option<String>,
    pub tags: Vec<(String, String)>,
}

impl RawImage {
    fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn from_sdk(i: &Image) -> Self {
        Self {
            id: i.image_id().map(str::to_string),
            name: i.name().map(str::to_string),
            state: i.state().map(|s| s.as_str().to_string()),
            architecture: i.architecture().map(|a| a.as_str().to_string()),
            creation_date: i.creation_date().map(str::to_string),
            tags: i
                .tags()
                .iter()
                .filter_map(|t| {
                    t.key()
                        .map(|k| (k.to_string(), t.value().unwrap_or("").to_string()))
                })
                .collect(),
        }
    }
}

pub(crate) fn validate_app_version(app_version: &str) -> Result<()> {
    if !VERSION_RE.is_match(app_version) {
        return Err(Error::invalid(format!(
            "app version {} is not a plain version string",
            json_str(app_version)
        )));
    }
    Ok(())
}

/// The newest image of each kind whose `ratchet:image_version` matches `app_version`, out of a DescribeImages-like list.
/// Shared by the EC2 lookup and the mock so both apply the same rules.
pub fn select_images(images: &[RawImage], app_version: &str, arch: RatchetArch) -> FoundImages {
    let mut out = FoundImages::default();
    for kind in [RatchetImageKind::Runner, RatchetImageKind::Bench] {
        let of_kind: Vec<&RawImage> = images
            .iter()
            .filter(|i| {
                i.tag(TAG_IMAGE) == Some(kind.as_str())
                    && i.id.is_some()
                    && i.state.as_deref().unwrap_or("available") == "available"
            })
            .collect();
        let mut matching: Vec<&RawImage> = of_kind
            .iter()
            .copied()
            .filter(|i| image_version_matches(i.tag(TAG_IMAGE_VERSION).unwrap_or(""), app_version))
            .collect();
        // ISO-8601 timestamps sort lexicographically; newest first (stable, so ties keep the listing order).
        matching.sort_by(|a, b| {
            b.creation_date
                .as_deref()
                .unwrap_or("")
                .cmp(a.creation_date.as_deref().unwrap_or(""))
        });
        let best = matching.first().map(|b| RatchetImage {
            id: b.id.clone().unwrap_or_default(),
            name: b.name.clone().unwrap_or_default(),
            version: b.tag(TAG_IMAGE_VERSION).unwrap_or("").to_string(),
            arch: b
                .architecture
                .clone()
                .unwrap_or_else(|| arch.as_str().to_string()),
            created_at: b.creation_date.clone().unwrap_or_default(),
        });
        let others: Vec<String> = of_kind
            .iter()
            .map(|i| i.tag(TAG_IMAGE_VERSION).unwrap_or("(untagged)").to_string())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        match kind {
            RatchetImageKind::Runner => {
                out.runner = best;
                out.other_versions.runner = others;
            }
            RatchetImageKind::Bench => {
                out.bench = best;
                out.other_versions.bench = others;
            }
        }
    }
    out
}

/// `DescribeImages` (own account only unless `owners` says otherwise) for images tagged `ratchet:image=runner|bench`.
#[derive(Clone)]
pub struct Ec2ImageLookup {
    client: aws_sdk_ec2::Client,
    owners: Vec<String>,
}

impl Ec2ImageLookup {
    pub fn new(client: aws_sdk_ec2::Client) -> Self {
        Self {
            client,
            owners: vec!["self".to_string()],
        }
    }

    /// Accounts that own the images. Default `self`: the sandbox account, where Packer built them.
    pub fn owners(mut self, owners: Vec<String>) -> Self {
        self.owners = owners;
        self
    }

    pub async fn connect(region: &str, profile: Option<&str>) -> Self {
        let cfg = crate::compute::aws::load_sdk_config(region, profile).await;
        Self::new(aws_sdk_ec2::Client::new(&cfg))
    }
}

#[async_trait]
impl ImageLookup for Ec2ImageLookup {
    async fn find_images(&self, app_version: &str, arch: RatchetArch) -> Result<FoundImages> {
        validate_app_version(app_version)?;
        let res = self
            .client
            .describe_images()
            .set_owners(Some(self.owners.clone()))
            .filters(
                Filter::builder()
                    .name(format!("tag:{TAG_IMAGE}"))
                    .values("runner")
                    .values("bench")
                    .build(),
            )
            .filters(
                Filter::builder()
                    .name("state")
                    .values(ImageState::Available.as_str())
                    .build(),
            )
            .filters(
                Filter::builder()
                    .name("architecture")
                    .values(arch.as_str())
                    .build(),
            )
            .send()
            .await
            .map_err(sdk)?;
        let raw: Vec<RawImage> = res.images().iter().map(RawImage::from_sdk).collect();
        Ok(select_images(&raw, app_version, arch))
    }
}

/// In-memory `ImageLookup` for simulation and tests: applies the same selection rules to a canned image list and records
/// the lookups.
#[derive(Default)]
pub struct MockImageLookup {
    images: Vec<RawImage>,
    calls: Mutex<Vec<(String, RatchetArch)>>,
}

impl MockImageLookup {
    pub fn new(images: Vec<RawImage>) -> Self {
        Self {
            images,
            calls: Mutex::default(),
        }
    }

    /// One runner and one bench image of `version`, with plausible ids: what a simulation needs.
    pub fn with_version(version: &str) -> Self {
        let image = |kind: &str, id: &str| RawImage {
            id: Some(id.to_string()),
            name: Some(format!("ratchet-{kind}-{version}-x86_64")),
            state: Some("available".to_string()),
            architecture: Some("x86_64".to_string()),
            creation_date: Some("2026-01-01T00:00:00.000Z".to_string()),
            tags: vec![
                (TAG_IMAGE.to_string(), kind.to_string()),
                (TAG_IMAGE_VERSION.to_string(), version.to_string()),
            ],
        };
        Self::new(vec![
            image("runner", "ami-0mockrunner0001"),
            image("bench", "ami-0mockbench00001"),
        ])
    }

    pub fn calls(&self) -> Vec<(String, RatchetArch)> {
        self.calls.lock().expect("calls poisoned").clone()
    }
}

#[async_trait]
impl ImageLookup for MockImageLookup {
    async fn find_images(&self, app_version: &str, arch: RatchetArch) -> Result<FoundImages> {
        validate_app_version(app_version)?;
        self.calls
            .lock()
            .expect("calls poisoned")
            .push((app_version.to_string(), arch));
        let arch_name = arch.as_str();
        let in_arch: Vec<RawImage> = self
            .images
            .iter()
            .filter(|i| i.architecture.as_deref().is_none_or(|a| a == arch_name))
            .cloned()
            .collect();
        Ok(select_images(&in_arch, app_version, arch))
    }
}

/// Thrown when an image the app needs is not published for this version, with what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLookupError {
    pub kind: RatchetImageKind,
    pub version: String,
    pub region: String,
    pub arch: String,
    pub found: Vec<String>,
}

impl fmt::Display for ImageLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = self.kind.as_str();
        write!(
            f,
            "no Ratchet {kind} image for version {} ({}) in {} owned by this account{}. Build it with infra/images/build.sh {kind} (it tags the AMI ratchet:image={kind} and ratchet:image_version), in the sandbox account and region, or set the AMI id explicitly under Settings > Advanced (development only).",
            self.version,
            self.arch,
            self.region,
            if self.found.is_empty() {
                "; found none".to_string()
            } else {
                format!("; found versions: {}", self.found.join(", "))
            },
        )
    }
}

impl std::error::Error for ImageLookupError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource {
    Override,
    Tag,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageSources {
    pub runner: Option<ImageSource>,
    pub bench: Option<ImageSource>,
}

/// The AMI ids to launch, and where each came from (for the Environments screen).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedImages {
    pub runner: Option<String>,
    pub bench: Option<String>,
    pub source: ImageSources,
}

#[derive(Debug, Clone, Default)]
pub struct ImageOverride {
    pub runner: Option<String>,
    pub bench: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolveImageArgs {
    pub app_version: String,
    pub region: String,
    /// Which kinds the caller will launch. Default: both.
    pub need: Option<Vec<RatchetImageKind>>,
    pub override_ids: ImageOverride,
    pub arch: Option<RatchetArch>,
}

impl ResolveImageArgs {
    pub fn new(app_version: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            region: region.into(),
            need: None,
            override_ids: ImageOverride::default(),
            arch: None,
        }
    }
}

/// The AMI ids to launch: an explicit override wins, otherwise the tag lookup (only for the kinds that are needed and
/// not overridden). A missing needed image is an [`ImageLookupError`].
pub async fn resolve_image_refs(
    lookup: &dyn ImageLookup,
    a: &ResolveImageArgs,
) -> Result<ResolvedImages> {
    let need = a
        .need
        .clone()
        .unwrap_or_else(|| vec![RatchetImageKind::Runner, RatchetImageKind::Bench]);
    let mut out = ResolvedImages::default();
    let mut missing = vec![];
    for kind in need {
        let o = match kind {
            RatchetImageKind::Runner => a.override_ids.runner.as_deref(),
            RatchetImageKind::Bench => a.override_ids.bench.as_deref(),
        };
        match o.filter(|o| !o.is_empty()) {
            Some(o) => {
                if !AMI_RE.is_match(o) {
                    return Err(Error::invalid(format!(
                        "the {} AMI override {} is not an ami-... id",
                        kind.as_str(),
                        json_str(o)
                    )));
                }
                set(&mut out, kind, o.to_string(), ImageSource::Override);
            }
            None => missing.push(kind),
        }
    }
    if missing.is_empty() {
        return Ok(out);
    }
    let arch = a.arch.unwrap_or_default();
    let found = lookup.find_images(&a.app_version, arch).await?;
    for kind in missing {
        let (hit, others) = match kind {
            RatchetImageKind::Runner => (found.runner.as_ref(), &found.other_versions.runner),
            RatchetImageKind::Bench => (found.bench.as_ref(), &found.other_versions.bench),
        };
        let Some(hit) = hit else {
            return Err(ImageLookupError {
                kind,
                version: a.app_version.clone(),
                region: a.region.clone(),
                arch: arch.as_str().to_string(),
                found: others.clone(),
            }
            .into());
        };
        set(&mut out, kind, hit.id.clone(), ImageSource::Tag);
    }
    Ok(out)
}

fn set(out: &mut ResolvedImages, kind: RatchetImageKind, id: String, source: ImageSource) {
    match kind {
        RatchetImageKind::Runner => {
            out.runner = Some(id);
            out.source.runner = Some(source);
        }
        RatchetImageKind::Bench => {
            out.bench = Some(id);
            out.source.bench = Some(source);
        }
    }
}
