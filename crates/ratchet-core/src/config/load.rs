//! Reading `ratchet.yaml`: YAML syntax errors, empty and non-mapping documents, then validation.

use std::path::Path;

use serde_norway::Value;

use super::parse::{ConfigError, validate};
use super::schema::RatchetConfig;

/// The outcome of parsing: a validated config, or every problem found.
pub type ConfigResult = Result<RatchetConfig, Vec<ConfigError>>;

/// Format an error as a single human-readable line.
pub fn format_config_error(e: &ConfigError) -> String {
    e.to_string()
}

fn whole_document(message: impl Into<String>) -> Vec<ConfigError> {
    vec![ConfigError {
        path: String::new(),
        message: message.into(),
    }]
}

/// Parse and validate `ratchet.yaml` text. Never panics for bad input.
pub fn parse_config(text: &str) -> ConfigResult {
    let doc: Value = match serde_norway::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            let mut message = e.to_string();
            // serde_norway appends " at line L column C"; the position is reported up front instead
            if let Some(i) = message.find(" at line ") {
                message.truncate(i);
            }
            let message = message.lines().next().unwrap_or("").to_owned();
            let at = e
                .location()
                .map(|l| format!(" (line {}, column {})", l.line(), l.column()))
                .unwrap_or_default();
            return Err(whole_document(format!("invalid YAML{at}: {message}")));
        }
    };
    match &doc {
        Value::Null => Err(whole_document("ratchet.yaml is empty")),
        Value::Mapping(_) => validate(&doc),
        _ => Err(whole_document(
            "ratchet.yaml must be a mapping at the top level",
        )),
    }
}

/// Read and validate a ratchet.yaml file. A missing or unreadable file is reported as an error.
pub fn load_config(path: impl AsRef<Path>) -> ConfigResult {
    let path = path.as_ref();
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(whole_document(format!("cannot find {}", path.display())))
        }
        Err(e) => Err(whole_document(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}
