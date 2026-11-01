//! The `ratchet` command line.
//!
//! A thin client of the `ratchetd` socket API (`list`, `run`, `status`, `stop`, `report`, `events`, ...) plus the
//! offline commands that only need `ratchet-core` (`validate`, `init`, `baseline`, `templates`). Every command takes
//! `--json`, so agents and scripts drive exactly what the Mac app shows. Port of `packages/cli`; see `PARITY.md`.

pub mod cli;
pub mod client;
pub mod commands;
pub mod io;

use clap::Parser;

use crate::cli::Cli;
use crate::io::Io;

/// Run the CLI with `argv` (including the program name). Returns the process exit code: 0 success, 1 failure,
/// 2 usage error.
pub async fn run(argv: Vec<String>, io: &dyn Io) -> i32 {
    let cli = match Cli::try_parse_from(&argv) {
        Ok(c) => c,
        Err(e) => {
            let text = e.render().to_string();
            let code = e.exit_code();
            if code == 0 {
                io.out(&text);
            } else {
                io.err(&text);
            }
            return code;
        }
    };
    commands::dispatch(cli, io).await
}
