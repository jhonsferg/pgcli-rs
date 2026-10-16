//! Execution of a plain SQL line in the REPL: interactive COPY FROM STDIN,
//! Ctrl-C cancellation, result rendering and automatic reconnection.

use std::io::Write as _;

use pgcli_core::error::PgCliError;
use pgcli_executor::query::QueryExecutor;
use pgcli_output::formats::{format_result, OutputFormat};
use pgcli_repl::schema_cache::SchemaCache;

use crate::session::Session;
use crate::settings::effective_expanded;

impl Session<'_> {
    /// Runs one line of SQL typed at the prompt and prints its results.
    pub async fn run_sql(&mut self, trimmed: &str) {
        // Interactive COPY FROM STDIN: read lines until \. then send via copy_in.
        let upper = trimmed.to_ascii_uppercase();
        if upper.contains("COPY") && upper.contains("FROM STDIN") {
            use futures_util::SinkExt;
            println!("Enter data (one row per line, end with \\.):");
            let mut copy_data = Vec::<u8>::new();
            let mut row_count: u64 = 0;
            while let Ok(Some(line)) = self.editor.readline() {
                if line.trim() == "\\." {
                    break;
                }
                copy_data.extend_from_slice(line.as_bytes());
                copy_data.push(b'\n');
                row_count += 1;
            }
            match self.pool.client().copy_in(trimmed).await {
                Ok(sink) => {
                    futures_util::pin_mut!(sink);
                    let chunk = bytes::Bytes::from(copy_data);
                    if let Err(e) = sink.send(chunk).await {
                        eprintln!("COPY error: {e}");
                    } else {
                        match sink.close().await {
                            Ok(_) => println!("COPY {row_count}"),
                            Err(e) => eprintln!("COPY finish error: {e}"),
                        }
                    }
                }
                Err(e) => eprintln!("ERROR: {e}"),
            }
            return;
        }

        if self.args.echo_queries {
            println!(">> {trimmed}");
        }

        // Apply variable substitution before execution.
        let sql = self.dispatcher.substitute_vars(trimmed);

        // Capture cancel token before borrowing self.pool for the query.
        let cancel_token = self.pool.client().cancel_token();

        // Show spinner on TTY for queries that may take time.
        let spinner = pgcli_output::Spinner::start("running...");

        // Race query against Ctrl-C: on interrupt, send a cancel request to the server.
        let exec_result = tokio::select! {
            biased;
            r = QueryExecutor::execute_batch(self.pool.client(), &sql) => r,
            _ = tokio::signal::ctrl_c() => {
                let _ = cancel_token.cancel_query(tokio_postgres::NoTls).await;
                Err(PgCliError::Interrupted)
            }
        };

        if let Some(sp) = spinner {
            sp.stop();
        }

        // Update transaction status indicator for the prompt.
        if let Ok(txn_row) = QueryExecutor::execute(
            self.pool.client(),
            "SELECT CASE pg_current_xact_id_if_assigned() IS NOT NULL WHEN true THEN '*' ELSE '' END",
        ).await {
            if let Some(row) = txn_row.rows.first() {
                if let Some(val) = row.values.first() {
                    self.editor.txn_status = val.to_string();
                }
            }
        }

        match exec_result {
            Ok(results) => {
                // Remember this SQL for \watch, \p, \bench, \gset.
                self.dispatcher.last_sql = sql.trim_end_matches(';').trim().to_string();
                for result in results {
                    self.opts.expanded = effective_expanded(&self.dispatcher, &result);
                    self.opts.timing = self.dispatcher.timing;

                    // For zero-column results (VACUUM, ANALYZE, SET, etc.), show command tag.
                    let out = if result.columns.is_empty() {
                        let tag = &result.command_tag;
                        let timing = if self.opts.timing {
                            format!(
                                " - {}",
                                pgcli_output::formats::format_duration(result.duration_ms)
                            )
                        } else {
                            String::new()
                        };
                        format!("{tag}{timing}")
                    } else if result.command_tag == "EXPLAIN" && self.format == OutputFormat::Table
                    {
                        // EXPLAIN results: apply colorized plan renderer instead of table.
                        let plan_lines: Vec<String> = result
                            .rows
                            .iter()
                            .map(|r| r.values.first().map(|v| v.to_string()).unwrap_or_default())
                            .collect();
                        pgcli_output::formats::colorize_explain_plan(
                            &plan_lines,
                            std::io::IsTerminal::is_terminal(&std::io::stdout()),
                        )
                    } else {
                        let table_out =
                            format_result(&result, &self.format, &self.opts).unwrap_or_default();
                        // For DML with RETURNING, also emit the command tag (psql compat).
                        let verb = result.command_tag.split_whitespace().next().unwrap_or("");
                        if matches!(verb, "INSERT" | "UPDATE" | "DELETE") {
                            format!("{}{}", table_out, result.command_tag)
                        } else {
                            table_out
                        }
                    };

                    // \o file redirect.
                    if let Some(ref out_path) = self.dispatcher.output_file {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(out_path)
                        {
                            writeln!(f, "{out}").ok();
                        }
                    } else {
                        self.pager.print(&out).ok();
                    }
                }
            }
            Err(PgCliError::Interrupted) => {
                eprintln!("\nQuery cancelled.");
            }
            Err(e) => {
                let msg = e.to_string();
                // Server-side cancellation message is printed more cleanly.
                if msg.contains("canceling statement due to user request") {
                    eprintln!("Query cancelled by user.");
                } else {
                    eprintln!("ERROR: {e}");
                    if self.args.echo_errors {
                        eprintln!("Failed: {trimmed}");
                    }
                    // Auto-reconnect on connection loss.
                    if !self.pool.is_alive().await {
                        eprintln!("Connection to server was lost. Reconnecting...");
                        match self.pool.reconnect().await {
                            Ok(()) => {
                                eprintln!("Reconnected to \"{}\".", self.pool.config().database);
                                if let Err(ce) =
                                    SchemaCache::refresh(&self.schema_cache, self.pool.client())
                                        .await
                                {
                                    tracing::warn!("Schema cache refresh after reconnect: {ce}");
                                }
                            }
                            Err(re) => {
                                eprintln!("Reconnect failed: {re}");
                                eprintln!(
                                    "Connection to \"{}\" could not be restored. \
                                     Use \\reconnect to retry or \\q to exit.",
                                    self.pool.config().database
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
