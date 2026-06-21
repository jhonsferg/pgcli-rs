/// pgcli-rs-A self-contained PostgreSQL CLI client in pure Rust.
///
/// This crate provides both the library used by the `pgcli-rs` binary and
/// a public API for embedding PostgreSQL connectivity in other programs.
pub mod cli;
pub mod connection;
pub mod error;
pub mod executor;
pub mod export;
pub mod meta;
pub mod output;
pub mod protocol;
pub mod repl;
pub mod scripting;

pub use error::{PgCliError, Result};
