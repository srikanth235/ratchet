//! The desktop main-process logic the daemon needs, ported from `packages/desktop/src/main/`: where secrets live
//! ([`secrets`]), what the live runtime is configured with ([`live_config`]), the detected agent CLIs ([`harnesses`]) and
//! the keep-awake hook ([`power`]).

pub mod harnesses;
pub mod live_config;
pub mod power;
pub mod product;
pub mod secrets;
#[cfg(feature = "testing")]
pub mod testing;

pub use harnesses::{Detected, HarnessCache, HarnessProbe, RealHarnessProbe};
pub use live_config::{
    DefaultPathsInput, LivePaths, SecretLookup, default_live_paths, effective_paths, secret_source,
    setting_for_secret_env,
};
pub use power::{NoPowerBlocker, PowerSaveBlocker, PowerSaveHook};
pub use product::{
    ImageLookupFailure, ImageQuery, ImageResolver, LiveAppConfig, LiveFactory, LiveHandle,
    LiveSettingsSource, LiveSnapshot, ProductEngine, ProductOptions, RuntimeEngine, SimFactory,
    SimHandle, demo_project,
};
#[cfg(target_os = "macos")]
pub use secrets::KeychainSecretStore;
pub use secrets::{
    FileSecretStore, KEYCHAIN_SERVICE, MemorySecretStore, SecretError, SecretStore,
    default_secret_store,
};
