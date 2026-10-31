//! Mappers: ledger rows -> the view models of the API. Pure functions and [`LedgerViews`].
//!
//! Port of `packages/desktop/src/main/{mappers,kpis,value,metrics}.ts`.

pub mod format;
pub mod kpis;
pub mod ledger_views;
pub mod metrics;
pub mod value;

pub use crate::api::summary::validate_config_text;
pub use ledger_views::{LedgerViews, PrUrlLookup};
