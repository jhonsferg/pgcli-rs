//! pgcli entry point.
//!
//! Parses CLI arguments, initializes tracing and hands over to [`app::run`],
//! which builds the connection config and dispatches to the interactive REPL
//! or to a non-interactive mode.

mod app;
mod copy;
mod crosstab;
mod export;
mod meta_handler;
mod oneshot;
mod pset;
mod session;
mod settings;
mod shell;
mod sql;
mod upgrade;

use std::process;

use clap::Parser;
use tracing::error;
use tracing_subscriber::{fmt, EnvFilter};

use pgcli_cli::CliArgs;
use pgcli_core::error::PgCliError;

#[tokio::main]
async fn main() {
    upgrade::remove_stale_old();
    let args = CliArgs::parse();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if args.quiet {
            EnvFilter::new("error")
        } else {
            EnvFilter::new("warn")
        }
    });
    fmt().with_env_filter(filter).init();

    if let Err(e) = app::run(args).await {
        if matches!(e, PgCliError::Interrupted) {
            process::exit(0);
        }
        error!("{e}");
        eprintln!("Error: {e}");
        process::exit(1);
    }
}
