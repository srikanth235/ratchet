//! Small shared HTTP helpers: one client, GitHub headers, and error mapping that never carries a token.
//!
//! The crate talks to GitHub with `reqwest` + `serde` rather than octocrab; see the crate docs for why.

use std::time::Duration;

use reqwest::{RequestBuilder, Response};

use crate::error::{Error, Result, scrub};

pub const API_VERSION: &str = "2022-11-28";
pub const DEFAULT_API_URL: &str = "https://api.github.com";
pub const DEFAULT_WEB_URL: &str = "https://github.com";
pub(crate) const TIMEOUT: Duration = Duration::from_secs(20);

/// The default client: the system proxy settings apply, every request times out after 20 s.
pub fn default_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Ratchet")
        .timeout(TIMEOUT)
        .build()
        .expect("reqwest client")
}

pub(crate) fn trim_base(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

/// A request to the REST API with the standard GitHub headers and `token` as a bearer credential.
pub(crate) fn authed(req: RequestBuilder, token: &str) -> RequestBuilder {
    req.header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", API_VERSION)
        .header("User-Agent", "Ratchet")
}

/// Send, mapping transport failures to a message that cannot contain `token`.
pub(crate) async fn send(req: RequestBuilder, token: &str) -> Result<Response> {
    req.send().await.map_err(|e| {
        Error::http(
            None,
            format!("Could not reach GitHub: {}", scrub(&describe(&e), token)),
        )
    })
}

pub(crate) fn describe(e: &reqwest::Error) -> String {
    // reqwest's Display includes the URL (which may carry a secret); describe the failure from the source chain
    let kind = if e.is_timeout() {
        "request timed out"
    } else if e.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    let mut msg = kind.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    msg
}

/// The `message` GitHub puts in an error body, if any.
pub(crate) async fn error_message(res: Response, token: &str) -> String {
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str().map(String::from)))
        .unwrap_or_else(|| {
            status
                .canonical_reason()
                .unwrap_or("request failed")
                .to_string()
        });
    scrub(&message, token)
}
