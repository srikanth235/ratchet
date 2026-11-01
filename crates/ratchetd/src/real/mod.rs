//! `RealService`: the [`AppService`](crate::service::AppService) of the real engine (`ratchet-engine`), plus the desktop
//! main-process logic it needs that depends on the API types: the settings store and the AWS profile reader. The
//! secret store, live configuration, harness cache and power hook are in `ratchet_engine::desktop`.

pub mod aws_profiles;
pub mod mappers;
pub mod service;
pub mod settings;

pub use aws_profiles::{
    AwsProfileList, ListAwsProfilesOptions, list_aws_profiles, parse_aws_files,
};
pub use service::{ConnectionProbe, RealService, RealServiceOptions};
pub use settings::SettingsStore;
