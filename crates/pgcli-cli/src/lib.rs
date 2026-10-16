//! pgcli-cli: command-line arguments and shell completions of the `pgcli`
//! binary. The binary itself (`main.rs` and its modules) is built on top of
//! this library and of the other pgcli crates.

pub mod args;
pub mod completions;

pub use args::CliArgs;
