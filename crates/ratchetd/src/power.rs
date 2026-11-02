//! The macOS power assertion that keeps the Mac awake while optimizations run agents on it (`IOPMAssertion`,
//! `PreventUserIdleSystemSleep`: the display may sleep, the machine may not).
//!
//! The daemon holds it only while a LOCAL agent runs (the engine's `run-active` event lists those runs; a cloud run goes on
//! without the Mac). If the assertion cannot be created, [`CommandPowerBlocker`](crate::host::CommandPowerBlocker) falls back to
//! `caffeinate -i -w <pid>`. The assertion belongs to the daemon process, so a crashed daemon releases it.
//!
//! The FFI is the smallest it can be: two IOKit calls and the CoreFoundation strings they take. Everything around it goes
//! through [`Assertions`], so the rules (one assertion per hold, released exactly once) are tested without IOKit.

use std::collections::HashMap;
use std::sync::Mutex;

/// Does this host have a way to keep itself awake? The daemon reports `keepingAwake` only where it does.
pub const KEEP_AWAKE_SUPPORTED: bool = cfg!(target_os = "macos");

/// What the power assertion needs from the OS.
pub trait Assertions: Send + Sync {
    /// Create an assertion that prevents idle system sleep; `reason` is what Activity Monitor shows. `Err` carries the
    /// `IOReturn`.
    fn create(&self, reason: &str) -> Result<u32, i32>;
    fn release(&self, id: u32);
}

/// IOKit's assertions on macOS; refuses everywhere else.
pub struct IoKitAssertions;

#[cfg(target_os = "macos")]
mod iokit {
    use std::ffi::{CString, c_char, c_void};

    type CFStringRef = *const c_void;
    type IoReturn = i32;
    type AssertionId = u32;

    const ASSERTION_LEVEL_ON: u32 = 255;
    const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPMAssertionCreateWithName(
            assertion_type: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut AssertionId,
        ) -> IoReturn;
        fn IOPMAssertionRelease(id: AssertionId) -> IoReturn;
    }

    fn cf_string(s: &str) -> Option<CFStringRef> {
        let c = CString::new(s).ok()?;
        // SAFETY: `c` is a valid NUL-terminated string for the duration of the call; a null allocator is the default one.
        let r = unsafe {
            CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        (!r.is_null()).then_some(r)
    }

    pub fn create(reason: &str) -> Result<u32, i32> {
        let kind = cf_string("PreventUserIdleSystemSleep").ok_or(-1)?;
        let Some(name) = cf_string(reason) else {
            // SAFETY: `kind` was created above and is released exactly once.
            unsafe { CFRelease(kind) };
            return Err(-1);
        };
        let mut id: AssertionId = 0;
        // SAFETY: both strings are live CFStrings and `id` is a valid out pointer; the strings are released after the call
        // (IOKit copies what it keeps).
        let rc = unsafe { IOPMAssertionCreateWithName(kind, ASSERTION_LEVEL_ON, name, &mut id) };
        unsafe {
            CFRelease(kind);
            CFRelease(name);
        }
        if rc == 0 { Ok(id) } else { Err(rc) }
    }

    pub fn release(id: u32) {
        // SAFETY: `id` came from `create`; releasing an id twice is harmless to IOKit (it returns an error).
        unsafe { IOPMAssertionRelease(id) };
    }
}

impl Assertions for IoKitAssertions {
    #[cfg(target_os = "macos")]
    fn create(&self, reason: &str) -> Result<u32, i32> {
        iokit::create(reason)
    }

    #[cfg(not(target_os = "macos"))]
    fn create(&self, _reason: &str) -> Result<u32, i32> {
        Err(-1)
    }

    #[cfg(target_os = "macos")]
    fn release(&self, id: u32) {
        iokit::release(id);
    }

    #[cfg(not(target_os = "macos"))]
    fn release(&self, _id: u32) {}
}

/// The assertions one blocker holds, by the handle `PowerSaveBlocker::start` gave out.
pub struct Held {
    api: Box<dyn Assertions>,
    reason: String,
    ids: Mutex<HashMap<u64, u32>>,
}

impl Held {
    pub fn new(api: Box<dyn Assertions>, reason: impl Into<String>) -> Self {
        Self {
            api,
            reason: reason.into(),
            ids: Mutex::new(HashMap::new()),
        }
    }

    /// Take an assertion for `handle`. False when the OS refused (the caller falls back to another way).
    pub fn take(&self, handle: u64) -> bool {
        match self.api.create(&self.reason) {
            Ok(id) => {
                self.ids.lock().expect("lock").insert(handle, id);
                true
            }
            Err(rc) => {
                tracing::warn!(rc, "cannot create the power assertion");
                false
            }
        }
    }

    /// Release `handle`'s assertion; nothing happens the second time.
    pub fn drop_handle(&self, handle: u64) {
        if let Some(id) = self.ids.lock().expect("lock").remove(&handle) {
            self.api.release(id);
        }
    }

    pub fn count(&self) -> usize {
        self.ids.lock().expect("lock").len()
    }

    pub fn release_all(&self) {
        for (_, id) in self.ids.lock().expect("lock").drain() {
            self.api.release(id);
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::sync::Arc;
    use std::sync::Mutex;

    use super::Assertions;

    /// Records what the OS was asked, and can refuse.
    #[derive(Clone, Default)]
    pub struct FakeAssertions {
        pub log: Arc<Mutex<Vec<String>>>,
        pub refuse: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Assertions for FakeAssertions {
        fn create(&self, reason: &str) -> Result<u32, i32> {
            if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(-536870212);
            }
            let mut log = self.log.lock().unwrap();
            let id = log.iter().filter(|l| l.starts_with("create")).count() as u32 + 100;
            log.push(format!("create {id} {reason}"));
            Ok(id)
        }
        fn release(&self, id: u32) {
            self.log.lock().unwrap().push(format!("release {id}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeAssertions;
    use super::*;

    #[test]
    fn each_hold_takes_one_assertion_and_releases_it_once() {
        let api = FakeAssertions::default();
        let held = Held::new(Box::new(api.clone()), "agents running");
        assert!(held.take(1));
        assert!(held.take(2));
        assert_eq!(held.count(), 2);
        held.drop_handle(1);
        held.drop_handle(1); // idempotent
        assert_eq!(held.count(), 1);
        held.release_all();
        assert_eq!(held.count(), 0);
        let log = api.log.lock().unwrap().clone();
        assert_eq!(log.iter().filter(|l| l.starts_with("release")).count(), 2);
        assert!(log[0].ends_with("agents running"));
    }

    #[test]
    fn a_refusing_os_takes_nothing() {
        let api = FakeAssertions::default();
        api.refuse.store(true, std::sync::atomic::Ordering::SeqCst);
        let held = Held::new(Box::new(api), "x");
        assert!(!held.take(1));
        assert_eq!(held.count(), 0);
    }

    #[test]
    fn only_a_mac_reports_keeping_awake() {
        assert_eq!(KEEP_AWAKE_SUPPORTED, cfg!(target_os = "macos"));
    }
}
