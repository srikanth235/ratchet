//! Tests of the `ratchet` CLI: the ported TypeScript suites (validate, init, baseline, program wiring, cost
//! reconcile) and the commands that talk to an in-process daemon on a temp socket.

mod binary;
mod common;
mod daemon;
mod offline;
