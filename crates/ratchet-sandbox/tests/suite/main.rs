//! The ported test suites of `packages/aws`, as one test binary (each integration-test binary links the whole AWS SDK,
//! so a single target keeps build time and disk use down). One module per TypeScript test file; see PARITY.md.

mod artifacts;
mod common;
mod compute_aws;
mod compute_mock;
mod cost;
mod cost_explorer;
mod identity;
mod images;
mod pricing;
mod reaper;
mod sandbox_mock;
mod sandbox_override;
mod sandbox_tofu;
mod user_data;
