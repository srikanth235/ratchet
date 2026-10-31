//! Output sinks, injectable so commands can be tested without touching the process streams.

use std::io::Write;
use std::sync::Mutex;

pub trait Io: Send + Sync {
    fn out(&self, text: &str);
    fn err(&self, text: &str);
}

/// The real stdout / stderr.
pub struct ProcessIo;

impl Io for ProcessIo {
    fn out(&self, text: &str) {
        let mut o = std::io::stdout().lock();
        // a closed pipe (`ratchet list | head`) is not an error worth a panic
        let _ = o.write_all(text.as_bytes());
        let _ = o.flush();
    }

    fn err(&self, text: &str) {
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    }
}

/// An [`Io`] that records everything written to it.
#[derive(Default)]
pub struct BufferIo {
    stdout: Mutex<String>,
    stderr: Mutex<String>,
}

impl BufferIo {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stdout(&self) -> String {
        self.stdout
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn stderr(&self) -> String {
        self.stderr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl Io for BufferIo {
    fn out(&self, text: &str) {
        self.stdout
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_str(text);
    }

    fn err(&self, text: &str) {
        self.stderr
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_str(text);
    }
}
