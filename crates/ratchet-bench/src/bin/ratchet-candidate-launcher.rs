//! The setuid-root candidate launcher (see `ratchet_bench::launcher`).
//!
//! Build it static so the same binary works in any image of the bench's architecture:
//!
//! ```text
//! RUSTFLAGS='-C target-feature=+crt-static' cargo build --release -p ratchet-bench --bin ratchet-candidate-launcher \
//!     --target x86_64-unknown-linux-gnu        # or aarch64-unknown-linux-gnu
//! install -m 4755 -o root -g root target/<triple>/release/ratchet-candidate-launcher \
//!     /usr/local/libexec/ratchet/ratchet-candidate-ctr
//! ```
//!
//! The configuration path is fixed at `/ratchet/launcher.conf`: a setuid program must not take it from its caller.

use std::path::Path;

fn main() {
    ratchet_bench::launcher::main_with(Path::new(ratchet_bench::launcher::CONF_PATH));
}
