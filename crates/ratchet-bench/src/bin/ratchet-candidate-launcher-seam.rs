//! TEST SEAM ONLY. The candidate launcher with its configuration path taken from `RATCHET_LAUNCHER_TEST_CONF`, so that
//! `tests/launcher.rs` can run the real code path against a configuration file it controls. The real launcher
//! (`ratchet-candidate-launcher`) has the path compiled in because a setuid program must never take it from its caller. This
//! binary exists only with the `launcher-seam` feature (enabled for this crate's own tests); never build or install it for
//! production.

use std::path::PathBuf;

fn main() {
    let conf = PathBuf::from(std::env::var_os("RATCHET_LAUNCHER_TEST_CONF").unwrap_or_default());
    ratchet_bench::launcher::main_with(&conf);
}
