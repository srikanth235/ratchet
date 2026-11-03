//! Model catalog: the models each harness accepts, their aliases, defaults and token prices.
//!
//! The table is data. `data/models.json` is bundled into the engine; an app can ship a file with the same shape next to it
//! (`ModelCatalog::load`) so a new model or a price change needs no engine release. A model id the catalog does not know is
//! never refused: it passes through to the CLI with a warning ([`resolve_model`]).

use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::detect::HarnessKind;

const BUNDLED: &str = include_str!("../data/models.json");

/// What a model is picked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelUse {
    Chat,
    Optimization,
}

/// USD per million tokens at list price.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenPrice {
    pub input_per_m: f64,
    #[serde(default)]
    pub cached_input_per_m: Option<f64>,
    pub output_per_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub harness: HarnessKind,
    /// What is passed to the CLI's `--model`.
    pub id: String,
    pub label: String,
    /// Other names the CLI accepts for the same model.
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub default_for: Vec<ModelUse>,
    /// `None` when the CLI reports dollars itself (Claude Code, Grok) or the price is unknown.
    #[serde(default)]
    pub price: Option<TokenPrice>,
    #[serde(default)]
    pub context_window: Option<u64>,
}

impl ModelInfo {
    fn named(&self, name: &str) -> bool {
        self.id == name || self.aliases.iter().any(|a| a == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct CatalogFile {
    #[serde(default)]
    version: u32,
    models: Vec<ModelInfo>,
}

/// The result of looking a model id up.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModel {
    /// The id to hand to the CLI: always what the caller asked for.
    pub id: String,
    pub info: Option<ModelInfo>,
    /// Set when the catalog does not know the id.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelCatalog {
    models: Vec<ModelInfo>,
}

impl ModelCatalog {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let file: CatalogFile =
            serde_json::from_str(text).map_err(|e| format!("model catalog: {e}"))?;
        let catalog = Self {
            models: file.models,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// The catalog compiled into the engine.
    pub fn bundled() -> &'static ModelCatalog {
        static BUNDLED_CATALOG: OnceLock<ModelCatalog> = OnceLock::new();
        BUNDLED_CATALOG.get_or_init(|| {
            ModelCatalog::from_json(BUNDLED).expect("bundled models.json is a valid catalog")
        })
    }

    /// The bundled catalog, replaced by the file at `override_path` when it exists and parses. A broken file falls back to the
    /// bundled one and says why (the second value).
    pub fn load(override_path: Option<&Path>) -> (ModelCatalog, Option<String>) {
        let bundled = || Self::bundled().clone();
        let Some(path) = override_path else {
            return (bundled(), None);
        };
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::from_json(&text) {
                Ok(c) => (c, None),
                Err(e) => (
                    bundled(),
                    Some(format!(
                        "{}: {e}; using the bundled catalog",
                        path.display()
                    )),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (bundled(), None),
            Err(e) => (
                bundled(),
                Some(format!(
                    "{}: {e}; using the bundled catalog",
                    path.display()
                )),
            ),
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (i, m) in self.models.iter().enumerate() {
            if m.id.trim().is_empty() {
                return Err(format!("model catalog: entry {i} has no id"));
            }
            if self.models[..i]
                .iter()
                .any(|o| o.harness == m.harness && o.id == m.id)
            {
                return Err(format!(
                    "model catalog: {} lists {} twice",
                    m.harness.as_str(),
                    m.id
                ));
            }
        }
        for h in [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Grok,
        ] {
            for u in [ModelUse::Chat, ModelUse::Optimization] {
                let n = self
                    .models
                    .iter()
                    .filter(|m| m.harness == h && m.default_for.contains(&u))
                    .count();
                if n > 1 {
                    return Err(format!(
                        "model catalog: {} has {n} defaults for {u:?}",
                        h.as_str()
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn all(&self) -> &[ModelInfo] {
        &self.models
    }

    pub fn for_harness(&self, harness: HarnessKind) -> Vec<&ModelInfo> {
        self.models
            .iter()
            .filter(|m| m.harness == harness)
            .collect()
    }

    /// By id or alias.
    pub fn find(&self, harness: HarnessKind, name: &str) -> Option<&ModelInfo> {
        self.models
            .iter()
            .find(|m| m.harness == harness && m.named(name))
    }

    pub fn default_for(&self, harness: HarnessKind, use_: ModelUse) -> Option<&ModelInfo> {
        self.models
            .iter()
            .find(|m| m.harness == harness && m.default_for.contains(&use_))
    }

    /// Looks `name` up. An unknown id is not an error: it is returned as given, with a warning.
    pub fn resolve(&self, harness: HarnessKind, name: &str) -> ResolvedModel {
        let info = self.find(harness, name).cloned();
        let warning = info.is_none().then(|| {
            format!(
                "model {name:?} is not in the {} catalog; passing it to the CLI as is",
                harness.as_str()
            )
        });
        ResolvedModel {
            id: name.to_owned(),
            info,
            warning,
        }
    }

    /// The list price for a model (by id or alias), when the catalog has one.
    pub fn price_for(&self, harness: HarnessKind, name: &str) -> Option<TokenPrice> {
        self.find(harness, name).and_then(|m| m.price)
    }
}

/// Resolves `name` against the bundled catalog and logs the warning for an unknown id. Adapters call this where they build
/// `--model`.
pub fn resolve_model(harness: HarnessKind, name: &str) -> ResolvedModel {
    let r = ModelCatalog::bundled().resolve(harness, name);
    if let Some(w) = &r.warning {
        tracing::warn!("{w}");
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_catalog_parses_and_covers_every_harness() {
        let c = ModelCatalog::bundled();
        for h in [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Grok,
        ] {
            assert!(!c.for_harness(h).is_empty(), "{h:?} has models");
            for u in [ModelUse::Chat, ModelUse::Optimization] {
                assert!(
                    c.default_for(h, u).is_some(),
                    "{h:?} has a default for {u:?}"
                );
            }
        }
    }

    #[test]
    fn claude_code_defaults_to_opus_for_optimizations_and_sonnet_for_chat() {
        let c = ModelCatalog::bundled();
        let d = |u| {
            c.default_for(HarnessKind::ClaudeCode, u)
                .unwrap()
                .id
                .clone()
        };
        assert_eq!(d(ModelUse::Optimization), "opus");
        assert_eq!(d(ModelUse::Chat), "sonnet");
        for alias in ["opus", "sonnet", "haiku"] {
            assert!(c.find(HarnessKind::ClaudeCode, alias).is_some());
        }
        // a full id finds the same entry as its alias
        assert_eq!(
            c.find(HarnessKind::ClaudeCode, "claude-opus-5-5")
                .unwrap()
                .id,
            "opus"
        );
    }

    #[test]
    fn a_model_belongs_to_one_harness() {
        let c = ModelCatalog::bundled();
        assert!(c.find(HarnessKind::Codex, "gpt-5-codex").is_some());
        assert!(c.find(HarnessKind::Grok, "gpt-5-codex").is_none());
        assert!(c.find(HarnessKind::Grok, "grok-4").is_some());
    }

    #[test]
    fn codex_prices_come_from_the_catalog() {
        let p = ModelCatalog::bundled()
            .price_for(HarnessKind::Codex, "gpt-5-codex")
            .unwrap();
        assert_eq!(p.input_per_m, 1.25);
        assert_eq!(p.cached_input_per_m, Some(0.125));
        assert_eq!(p.output_per_m, 10.0);
        assert!(
            ModelCatalog::bundled()
                .price_for(HarnessKind::ClaudeCode, "opus")
                .is_none(),
            "Claude Code reports its own dollars"
        );
    }

    #[test]
    fn an_unknown_id_passes_through_with_a_warning() {
        let c = ModelCatalog::bundled();
        let r = c.resolve(HarnessKind::Codex, "gpt-9-ultra");
        assert_eq!(r.id, "gpt-9-ultra");
        assert!(r.info.is_none());
        assert!(r.warning.unwrap().contains("gpt-9-ultra"));
        let known = c.resolve(HarnessKind::Codex, "gpt-5-codex");
        assert!(known.warning.is_none() && known.info.is_some());
    }

    #[test]
    fn a_file_replaces_the_bundled_catalog_and_a_broken_one_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("models.json");
        std::fs::write(
            &good,
            r#"{"version":2,"models":[{"harness":"grok","id":"grok-9","label":"Grok 9","defaultFor":["chat","optimization"]}]}"#,
        )
        .unwrap();
        let (c, warn) = ModelCatalog::load(Some(&good));
        assert!(warn.is_none());
        assert_eq!(
            c.default_for(HarnessKind::Grok, ModelUse::Chat).unwrap().id,
            "grok-9"
        );
        assert!(c.for_harness(HarnessKind::Codex).is_empty());

        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        let (c, warn) = ModelCatalog::load(Some(&bad));
        assert!(warn.unwrap().contains("bundled"));
        assert_eq!(&c, ModelCatalog::bundled());

        let (c, warn) = ModelCatalog::load(Some(&dir.path().join("missing.json")));
        assert!(warn.is_none());
        assert_eq!(&c, ModelCatalog::bundled());
    }

    #[test]
    fn duplicate_ids_and_two_defaults_are_refused() {
        let dup = r#"{"models":[{"harness":"codex","id":"a","label":"A"},{"harness":"codex","id":"a","label":"A"}]}"#;
        assert!(ModelCatalog::from_json(dup).unwrap_err().contains("twice"));
        let two = r#"{"models":[
            {"harness":"codex","id":"a","label":"A","defaultFor":["chat"]},
            {"harness":"codex","id":"b","label":"B","defaultFor":["chat"]}]}"#;
        assert!(
            ModelCatalog::from_json(two)
                .unwrap_err()
                .contains("defaults")
        );
    }
}
