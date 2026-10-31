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
    /// A failure (`Err`) makes the whole probe an empty result.
    async fn login_path(&self) -> Result<Option<String>, String>;
    async fn detect(&self, login_path: Option<&str>) -> Result<Vec<HarnessInfo>, String>;
}

/// Probes this machine.
pub struct RealHarnessProbe;

#[async_trait]
impl HarnessProbe for RealHarnessProbe {
    async fn login_path(&self) -> Result<Option<String>, String> {
        Ok(Some(
            resolve_login_path(&DetectOptions::default()).await.path,
        ))
    }

    async fn detect(&self, login_path: Option<&str>) -> Result<Vec<HarnessInfo>, String> {
        Ok(detect_harnesses(&DetectOptions {
            login_path: login_path.map(str::to_owned),
            ..DetectOptions::default()
        })
        .await)
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
        let probe = async {
            let login_path = self.probe.login_path().await?;
            let harnesses = self.probe.detect(login_path.as_deref()).await?;
            Ok::<_, String>(Detected {
                harnesses,
                login_path,
            })
        };
        probe.await.unwrap_or_default()
    }
}

/// The connector store reads the detected CLIs through the same cache.
#[async_trait]
impl ratchet_connectors::HarnessSource for HarnessCache {
    async fn get(&self) -> Vec<HarnessInfo> {
        HarnessCache::get(self).await.harnesses
    }

    async fn refresh(&self) -> Vec<HarnessInfo> {
        HarnessCache::refresh(self).await.harnesses
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ratchet_agents::detect::{HarnessAuth, HarnessKind};

    use super::*;

    fn info(kind: HarnessKind, version: &str) -> HarnessInfo {
        HarnessInfo {
            kind,
            installed: true,
            version: Some(version.into()),
            path: Some(format!("/bin/{}", kind.as_str())),
            auth: HarnessAuth::SignedIn,
            hint: "ok".into(),
        }
    }

    #[derive(Default)]
    struct Counting(AtomicUsize);

    #[async_trait]
    impl HarnessProbe for Counting {
        async fn login_path(&self) -> Result<Option<String>, String> {
            Ok(Some("/usr/bin:/opt/bin".into()))
        }
        async fn detect(&self, login_path: Option<&str>) -> Result<Vec<HarnessInfo>, String> {
            let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            assert_eq!(login_path, Some("/usr/bin:/opt/bin"));
            Ok(vec![info(HarnessKind::ClaudeCode, &format!("{n}.0.0"))])
        }
    }

    struct Hung;

    #[async_trait]
    impl HarnessProbe for Hung {
        async fn login_path(&self) -> Result<Option<String>, String> {
            Err("shell hung".into())
        }
        async fn detect(&self, _: Option<&str>) -> Result<Vec<HarnessInfo>, String> {
            Ok(Vec::new())
        }
    }

    /// machine.test.ts › HarnessCache › probes once, caches, and probes again on refresh
    #[tokio::test]
    async fn probes_once_caches_and_probes_again_on_refresh() {
        let probe = Arc::new(Counting::default());
        let cache = HarnessCache::new(probe.clone());
        let a = cache.get().await;
        let b = cache.get().await;
        assert_eq!(probe.0.load(Ordering::SeqCst), 1);
        assert_eq!(a, b);
        assert_eq!(a.login_path.as_deref(), Some("/usr/bin:/opt/bin"));
        assert_eq!(a.harnesses[0].version.as_deref(), Some("1.0.0"));
        assert_eq!(
            cache.refresh().await.harnesses[0].version.as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            cache.get().await.harnesses[0].version.as_deref(),
            Some("2.0.0")
        );
    }

    /// machine.test.ts › HarnessCache › a failing probe is an empty result, not a crash
    #[tokio::test]
    async fn a_failing_probe_is_an_empty_result() {
        let cache = HarnessCache::new(Arc::new(Hung));
        assert_eq!(cache.get().await, Detected::default());
    }
}
