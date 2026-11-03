//! Where secret values live (the API keys and the GitHub token). The settings files never hold one; they only know
//! which are set. Port of the `SecretCipher` half of `packages/desktop/src/main/settings.ts`: Electron's `safeStorage`
//! becomes the macOS Keychain here, and a file in tests.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// The keychain service every Ratchet secret is filed under.
pub const KEYCHAIN_SERVICE: &str = "com.ratchet.app";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// No secure store on this machine: refuse to store the value in plaintext.
    #[error("OS keychain encryption is unavailable; refusing to store the secret in plaintext")]
    Unavailable,
    #[error("{0}")]
    Failed(String),
}

/// A place for named secret values. `account` is the setting's name (`anthropicApiKey`, ...). Values never appear in
/// errors or `Debug` output.
pub trait SecretStore: Send + Sync {
    /// False when nothing secure is available; `set` then fails with [`SecretError::Unavailable`].
    fn available(&self) -> bool;
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, account: &str, value: &str) -> Result<(), SecretError>;
    /// Deleting a secret that is not stored succeeds.
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// In memory only (tests; a daemon without a keychain).
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<BTreeMap<String, String>>,
    unavailable: bool,
}

impl MemorySecretStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that behaves like a machine without a keychain.
    pub fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }
}

impl SecretStore for MemorySecretStore {
    fn available(&self) -> bool {
        !self.unavailable
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        if self.unavailable {
            return Ok(None);
        }
        Ok(self.values.lock().expect("lock").get(account).cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        if self.unavailable {
            return Err(SecretError::Unavailable);
        }
        self.values
            .lock()
            .expect("lock")
            .insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.values.lock().expect("lock").remove(account);
        Ok(())
    }
}

/// A JSON file (mode 0600) of plaintext values. For tests and for machines without a keychain that opt in; the macOS app
/// uses [`KeychainSecretStore`].
pub struct FileSecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write(&self, map: &BTreeMap<String, String>) -> Result<(), SecretError> {
        let fail =
            |e: std::io::Error| SecretError::Failed(format!("cannot write the secrets file: {e}"));
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        let tmp = self.path.with_extension("tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).map_err(fail)?;
        f.write_all(
            serde_json::to_string_pretty(map)
                .unwrap_or_default()
                .as_bytes(),
        )
        .map_err(fail)?;
        drop(f);
        std::fs::rename(&tmp, &self.path).map_err(fail)
    }
}

impl SecretStore for FileSecretStore {
    fn available(&self) -> bool {
        true
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        let _g = self.lock.lock().expect("lock");
        Ok(self.read().get(account).cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        let _g = self.lock.lock().expect("lock");
        let mut m = self.read();
        m.insert(account.to_owned(), value.to_owned());
        self.write(&m)
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        let _g = self.lock.lock().expect("lock");
        let mut m = self.read();
        if m.remove(account).is_some() {
            self.write(&m)?;
        }
        Ok(())
    }
}

/// The macOS Keychain (generic passwords under [`KEYCHAIN_SERVICE`]).
#[cfg(target_os = "macos")]
pub struct KeychainSecretStore {
    service: String,
}

#[cfg(target_os = "macos")]
impl KeychainSecretStore {
    pub fn new() -> Self {
        Self::with_service(KEYCHAIN_SERVICE)
    }

    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for KeychainSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

/// `errSecItemNotFound`.
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainSecretStore {
    fn available(&self) -> bool {
        true
    }

    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        use security_framework::passwords::get_generic_password;
        match get_generic_password(&self.service, account) {
            Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(SecretError::Failed(format!(
                "keychain read failed ({})",
                e.code()
            ))),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        use security_framework::passwords::set_generic_password;
        set_generic_password(&self.service, account, value.as_bytes())
            .map_err(|e| SecretError::Failed(format!("keychain write failed ({})", e.code())))
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        use security_framework::passwords::delete_generic_password;
        match delete_generic_password(&self.service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(SecretError::Failed(format!(
                "keychain delete failed ({})",
                e.code()
            ))),
        }
    }
}

/// The store the daemon uses by default: the Keychain on macOS. Elsewhere there is no secure store, so secrets are refused
/// unless `fallback_file` (mode 0600, plaintext) is given explicitly.
pub fn default_secret_store(fallback_file: Option<PathBuf>) -> std::sync::Arc<dyn SecretStore> {
    #[cfg(target_os = "macos")]
    {
        let _ = fallback_file;
        std::sync::Arc::new(KeychainSecretStore::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        match fallback_file {
            Some(f) => std::sync::Arc::new(FileSecretStore::new(f)),
            None => std::sync::Arc::new(MemorySecretStore::unavailable()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exercise(s: &dyn SecretStore) {
        assert_eq!(s.get("a").unwrap(), None);
        s.set("a", "one").unwrap();
        s.set("b", "two").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("one"));
        s.set("a", "uno").unwrap();
        assert_eq!(s.get("a").unwrap().as_deref(), Some("uno"));
        s.delete("a").unwrap();
        s.delete("a").unwrap();
        assert_eq!(s.get("a").unwrap(), None);
        assert_eq!(s.get("b").unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn memory_store_round_trips() {
        exercise(&MemorySecretStore::new());
    }

    #[test]
    fn file_store_round_trips_and_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("secrets.json");
        let s = FileSecretStore::new(&path);
        exercise(&s);
        // a new instance reads what the first wrote
        assert_eq!(
            FileSecretStore::new(&path).get("b").unwrap().as_deref(),
            Some("two")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "group/other must have no access");
        }
    }

    #[test]
    fn an_unavailable_store_refuses_to_store() {
        let s = MemorySecretStore::unavailable();
        assert!(!s.available());
        assert_eq!(s.set("a", "x"), Err(SecretError::Unavailable));
        assert_eq!(s.get("a").unwrap(), None);
    }
}
