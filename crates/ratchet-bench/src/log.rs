//! Bounded combined log for one bench job (port of `log.ts`).

use std::sync::{Arc, Mutex};

use ratchet_core::ports::Clock;

use crate::shell::iso_timestamp;

#[derive(Default)]
struct Inner {
    head: String,
    tail: String,
    dropped: usize,
}

/// Bounded combined log for one bench job. Keeps the head and the tail when the cap is exceeded, so the start of the run
/// (checkout/setup) and the end (the failure) both survive. Cloning shares the log (the pipeline and its executor write to
/// the same one).
#[derive(Clone)]
pub struct BenchLog {
    inner: Arc<Mutex<Inner>>,
    clock: Arc<dyn Clock>,
    head_cap: usize,
    tail_cap: usize,
}

/// Number of UTF-16 code units, the unit `String.length` counts in JavaScript (and the caps are in).
fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.slice(0, n)` in UTF-16 units (a split surrogate pair becomes U+FFFD).
fn take16(s: &str, n: usize) -> String {
    String::from_utf16_lossy(&s.encode_utf16().take(n).collect::<Vec<_>>())
}

fn skip16(s: &str, n: usize) -> String {
    String::from_utf16_lossy(&s.encode_utf16().skip(n).collect::<Vec<_>>())
}

impl BenchLog {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_cap(clock, 2 * 1024 * 1024)
    }

    pub fn with_cap(clock: Arc<dyn Clock>, cap_chars: usize) -> Self {
        BenchLog {
            inner: Arc::new(Mutex::new(Inner::default())),
            clock,
            head_cap: cap_chars / 2,
            tail_cap: cap_chars / 2,
        }
    }

    pub fn write(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let Ok(mut g) = self.inner.lock() else { return };
        let mut text = text.to_owned();
        let head_len = len16(&g.head);
        if head_len < self.head_cap {
            let room = self.head_cap - head_len;
            let part = take16(&text, room);
            g.head.push_str(&part);
            text = skip16(&text, room);
            if text.is_empty() {
                return;
            }
        }
        g.tail.push_str(&text);
        let tail_len = len16(&g.tail);
        if tail_len > self.tail_cap * 2 {
            let cut = tail_len - self.tail_cap;
            g.dropped += cut;
            g.tail = skip16(&g.tail, cut);
        }
    }

    pub fn line(&self, text: &str) {
        if text.ends_with('\n') {
            self.write(text);
        } else {
            self.write(&format!("{text}\n"));
        }
    }

    /// `=== name ===` header with a timestamp.
    pub fn section(&self, name: &str) {
        self.line(&format!(
            "\n=== {name} @ {} ===",
            iso_timestamp(self.clock.now())
        ));
    }

    fn render(&self) -> String {
        let Ok(g) = self.inner.lock() else {
            return String::new();
        };
        let tail_len = len16(&g.tail);
        let tail = if tail_len > self.tail_cap {
            skip16(&g.tail, tail_len - self.tail_cap)
        } else {
            g.tail.clone()
        };
        let dropped = g.dropped + (tail_len - len16(&tail));
        if dropped > 0 {
            format!(
                "{}\n[... {dropped} characters truncated ...]\n{tail}",
                g.head
            )
        } else {
            format!("{}{}", g.head, tail)
        }
    }
}

impl std::fmt::Display for BenchLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}
