//! The detected agent CLIs, probed once and cached. Port of `packages/desktop/src/main/harnesses.ts`.

use std::sync::Arc;

use async_trait::async_trait;
use ratchet_agents::detect::{DetectOptions, HarnessInfo, detect_harnesses, resolve_login_path};
use tokio::sync::Mutex;

/// What was found: the harnesses and the login-shell PATH they were found with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Detected {
    pub harnesses: Vec<HarnessInfo>,
    pub login_path: Option<String>,
}

#[async_trait]
pub trait HarnessProbe: Send + Sync {
    async fn login_path(&self) -> Option<String>;
    async fn detect(&self, login_path: Option<&str>) -> Vec<HarnessInfo>;
}

/// Probes this machine.
pub struct RealHarnessProbe;

#[async_trait]
impl HarnessProbe for RealHarnessProbe {
    async fn login_path(&self) -> Option<String> {
        Some(resolve_login_path(&DetectOptions::default()).await.path)
    }

    async fn detect(&self, login_path: Option<&str>) -> Vec<HarnessInfo> {
        detect_harnesses(&DetectOptions {
            login_path: login_path.map(str::to_owned),
            ..DetectOptions::default()
        })
        .await
    }
}

/// The detected agent CLIs, probed on first use and on demand ("Re-detect"), cached, and handed to the hybrid runtime so
/// the engine does not probe again. A failed probe is an empty result, never a crash.
pub struct HarnessCache {
    probe: Arc<dyn HarnessProbe>,
    current: Mutex<Option<Detected>>,
}

impl Default for HarnessCache {
    fn default() -> Self {
        Self::new(Arc::new(RealHarnessProbe))
    }
}

impl HarnessCache {
    pub fn new(probe: Arc<dyn HarnessProbe>) -> Self {
        Self {
            probe,
            current: Mutex::new(None),
        }
    }

    /// The cached result; probes once if nothing was probed yet.
    pub async fn get(&self) -> Detected {
        let mut g = self.current.lock().await;
        if let Some(d) = g.as_ref() {
            return d.clone();
        }
        let d = self.run().await;
        *g = Some(d.clone());
        d
    }

    /// Probe again and replace the cache.
    pub async fn refresh(&self) -> Detected {
        let mut g = self.current.lock().await;
        let d = self.run().await;
        *g = Some(d.clone());
        d
    }

    async fn run(&self) -> Detected {
        let login_path = self.probe.login_path().await;
        let harnesses = self.probe.detect(login_path.as_deref()).await;
        Detected {
            harnesses,
            login_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Default)]
    struct Counting(AtomicUsize);

    #[async_trait]
    impl HarnessProbe for Counting {
        async fn login_path(&self) -> Option<String> {
            Some("/usr/bin".into())
        }
        async fn detect(&self, login_path: Option<&str>) -> Vec<HarnessInfo> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(login_path, Some("/usr/bin"));
            Vec::new()
        }
    }

    #[tokio::test]
    async fn probes_once_until_refreshed() {
        let probe = Arc::new(Counting::default());
        let cache = HarnessCache::new(probe.clone());
        assert_eq!(cache.get().await.login_path.as_deref(), Some("/usr/bin"));
        cache.get().await;
        assert_eq!(probe.0.load(Ordering::SeqCst), 1);
        cache.refresh().await;
        assert_eq!(probe.0.load(Ordering::SeqCst), 2);
    }
}
