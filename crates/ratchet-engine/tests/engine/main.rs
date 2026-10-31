//! Engine tests: ports of `packages/engine/src/*.test.ts` and `packages/runtime/src/*.test.ts` (one test binary; see
//! PARITY.md for the case-by-case mapping).

// Test fixtures hold std mutexes across `.await` on a single-threaded runtime, compare ids against `[id.clone()]`, and keep
// recording tuples inline: all deliberate in tests.
#![allow(
    clippy::type_complexity,
    clippy::await_holding_lock,
    clippy::cloned_ref_to_slice_refs
)]

mod common;
mod core_loop;
mod desktop_product;
mod local_agents;
mod reconcile;
mod runtime_bench_contract;
mod runtime_live;
mod runtime_live_checks;
mod runtime_local_agents;
mod runtime_simulation;
mod runtime_summary;
mod sandbox_auto;
mod sandbox_lifecycle;
mod simplified;
