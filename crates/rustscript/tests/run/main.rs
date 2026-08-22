//! End to end tests through the real binary with the `cargo check` gate skipped. The 2 `check_` tests
//! at the end run a real cargo and are ignored by default. Run them with `cargo test --test run
//! -- --ignored`.

mod basics;
mod common;
mod exits;
mod json;
mod system;
mod tokio_engine;
