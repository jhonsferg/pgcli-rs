//! Builds the connection config from the CLI arguments and dispatches to the
//! selected mode: `--list`, `--command`, `--file` or the interactive REPL.

use pgcli_cli::CliArgs;
use pgcli_connection::config::ConnectionConfig;

use crate::{oneshot, session};

/// Runs pgcli with the parsed CLI arguments.
///
/// # Errors
///
/// Returns the first connection, query or I/O error of the selected mode.
pub async fn run(args: CliArgs) -> pgcli_core::Result<()> {
    let mut conn_cfg = ConnectionConfig::from_args(&args.connection_args())?;

    if conn_cfg.password.is_none() && !args.no_password && args.password {
        conn_cfg.password = Some(prompt_password(&format!(
            "Password for user {}: ",
            conn_cfg.user
        ))?);
    }

    if args.list {
        return oneshot::run_list(&args, &conn_cfg).await;
    }
    if let Some(ref sql) = args.command {
        return oneshot::run_command(&args, &conn_cfg, sql).await;
    }
    if let Some(ref path) = args.file {
        return oneshot::run_file(&args, &conn_cfg, path).await;
    }
    session::run_repl(&args, conn_cfg).await
}

/// Prompt the user for a password without echoing input.
fn prompt_password(prompt: &str) -> pgcli_core::Result<String> {
    rpassword::prompt_password(prompt).map_err(|e| {
        pgcli_core::error::PgCliError::Connection(format!("password prompt failed: {e}"))
    })
}
