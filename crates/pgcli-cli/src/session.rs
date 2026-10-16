//! Interactive REPL: the session state and the read-eval-print loop.
//!
//! Backslash meta-commands are handled in [`crate::meta_handler`] and plain
//! SQL in [`crate::sql`], both as methods of [`Session`].

use pgcli_cli::CliArgs;
use pgcli_connection::{config::ConnectionConfig, pool::ConnectionPool};
use pgcli_core::error::PgCliError;
use pgcli_meta::commands::{MetaCommand, MetaCommandDispatcher};
use pgcli_output::{
    formats::{FormatOptions, OutputFormat},
    pager::Pager,
};
use pgcli_repl::{
    editor::ReplEditor,
    highlighter::SqlHighlighter,
    schema_cache::{SchemaCache, SharedSchemaCache},
};
use tracing::{error, info};

use crate::pset::load_startup_file;
use crate::settings::{build_format_opts, build_pager, seed_variables};

/// What the loop does after a meta-command.
pub enum Flow {
    /// Read the next line.
    Continue,
    /// Leave the REPL (`\q`).
    Quit,
}

/// Mutable state of an interactive session.
pub struct Session<'a> {
    /// Parsed CLI arguments.
    pub args: &'a CliArgs,
    /// Open connection (reconnects in place).
    pub pool: ConnectionPool,
    /// Meta-command state: variables, toggles, last SQL.
    pub dispatcher: MetaCommandDispatcher,
    /// Line editor with history and highlighting.
    pub editor: ReplEditor,
    /// Current output format (`\pset format`).
    pub format: OutputFormat,
    /// Current formatting options (`\pset`, `\x`, `\timing`).
    pub opts: FormatOptions,
    /// Pager used for results.
    pub pager: Pager,
    /// Schema cache shared with the highlighter.
    pub schema_cache: SharedSchemaCache,
    /// Whether an explicit transaction block (`BEGIN` ... `COMMIT`) may be
    /// open, so the prompt indicator needs to ask the server.
    pub txn_block_open: bool,
}

/// Runs the interactive REPL until `\q` or end of input.
///
/// # Errors
///
/// Returns an error if the first connection or the editor cannot be set up.
/// Errors of individual statements are printed and the loop continues.
pub async fn run_repl(args: &CliArgs, conn_cfg: ConnectionConfig) -> pgcli_core::Result<()> {
    info!("Starting interactive REPL");
    let pool = ConnectionPool::connect(&conn_cfg).await?;
    let dbname = conn_cfg.database.clone();

    // Build schema cache and do an initial refresh in the background.
    // Load the completion cache in the background: the prompt does not wait
    // for the catalog queries.
    let schema_cache = SchemaCache::new_shared();
    SchemaCache::spawn_refresh(schema_cache.clone(), pool.shared_client());

    let highlighter = SqlHighlighter::new(&args.theme).with_cache(schema_cache.clone());
    let history_path = args
        .history_file
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.join(".pgcli_history")));

    let editor = ReplEditor::new(
        &dbname,
        &conn_cfg.user,
        &conn_cfg.host,
        conn_cfg.port,
        false,
        highlighter,
        history_path,
    )?;
    let mut dispatcher = MetaCommandDispatcher::new();
    dispatcher.theme = args.theme.clone();

    // Seed variables from -v NAME=VALUE flags.
    seed_variables(&mut dispatcher, args);

    let mut format = args.format.parse::<OutputFormat>().unwrap_or_default();
    let mut opts = build_format_opts(args);
    let pager = build_pager(args);

    // Load startup file (~/.pgclirc) and apply \set / \pset directives.
    load_startup_file(&mut dispatcher, &mut format, &mut opts);

    println!(
        "pgcli {version} - connected to {db} at {host}:{port}",
        version = env!("CARGO_PKG_VERSION"),
        db = dbname,
        host = conn_cfg.host,
        port = conn_cfg.port,
    );
    println!("Type \\? for help. Type \\q or press Ctrl-D to exit.\n");

    let mut session = Session {
        args,
        pool,
        dispatcher,
        editor,
        format,
        opts,
        pager,
        schema_cache,
        txn_block_open: false,
    };

    loop {
        let input = match session.editor.readline() {
            Ok(Some(line)) => line,
            Ok(None) => {
                println!("\nBye.");
                break;
            }
            Err(PgCliError::Interrupted) => {
                println!();
                continue;
            }
            Err(e) => {
                error!("{e}");
                continue;
            }
        };

        let trimmed = input.trim();

        if trimmed.starts_with('\\') {
            if let Some(cmd) = MetaCommand::parse(trimmed) {
                let result = session.dispatcher.dispatch(&cmd);
                if let Flow::Quit = session.handle_meta(result).await {
                    break;
                }
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        session.run_sql(trimmed).await;
    }

    session.editor.save_history().ok();
    Ok(())
}
