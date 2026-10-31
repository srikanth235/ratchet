//! Shared helpers for the integration tests. Real AWS is never called: SDK clients are built with
//! `aws-smithy-mocks` rules, and request inputs are captured by the rules' matchers.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use aws_smithy_types::error::metadata::ErrorMetadata;

/// Collects every value pushed to it; cloneable handle.
#[derive(Debug)]
pub struct Log<T>(Arc<Mutex<Vec<T>>>);

impl<T> Clone for Log<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Log<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

impl<T: Clone> Log<T> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&self, v: T) {
        self.0.lock().unwrap().push(v);
    }
    pub fn all(&self) -> Vec<T> {
        self.0.lock().unwrap().clone()
    }
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn first(&self) -> T {
        self.0
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("nothing was recorded")
    }
    pub fn last(&self) -> T {
        self.0
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("nothing was recorded")
    }
}

/// A matcher that records the request input and matches every request of its operation.
pub fn rec<I: Clone + Send + Sync + 'static>(
    log: &Log<I>,
) -> impl Fn(&I) -> bool + Send + Sync + 'static {
    let log = log.clone();
    move |i: &I| {
        log.push(i.clone());
        true
    }
}

/// Like [`rec`] for inputs that are not `Clone` (bodies): records what `f` extracts.
pub fn rec_with<I: 'static, T: Clone + Send + 'static>(
    log: &Log<T>,
    f: impl Fn(&I) -> T + Send + Sync + 'static,
) -> impl Fn(&I) -> bool + Send + Sync + 'static {
    let log = log.clone();
    move |i: &I| {
        log.push(f(i));
        true
    }
}

/// A matcher that records only the operation name (for ordering assertions across operations).
pub fn rec_op<I: 'static>(
    log: &Log<&'static str>,
    name: &'static str,
) -> impl Fn(&I) -> bool + Send + Sync + 'static {
    let log = log.clone();
    move |_: &I| {
        log.push(name);
        true
    }
}

pub fn meta(code: &str, message: &str) -> ErrorMetadata {
    ErrorMetadata::builder().code(code).message(message).build()
}
