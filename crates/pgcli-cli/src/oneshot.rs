//! Non-interactive modes: `--list`, `--command` and `--file`.

use std::path::Path;

use pgcli_cli::CliArgs;
use pgcli_connection::{config::ConnectionConfig, pool::ConnectionPool};
use pgcli_executor::{query::QueryExecutor, runner::ScriptRunner};
use pgcli_meta::{
    commands::{MetaCommand, MetaCommandDispatcher, MetaResult},
    introspection::{Introspector, ObjectType},
};
use pgcli_output::{
    formats::{format_result, OutputFormat},
    stats::{estimate_result_bytes, BenchStats},
};

use crate::export::export_to_file;
use crate::settings::{
    build_format_opts, build_pager, seed_variables, seed_variables_into_runner, write_output,
    write_output_or_pager,
};

/// `--list`: prints the databases of the server.
pub async fn run_list(args: &CliArgs, conn_cfg: &ConnectionConfig) -> pgcli_core::Result<()> {
    let pool = ConnectionPool::connect(conn_cfg).await?;
    let sql = "SELECT datname AS \"Name\", \
                   pg_catalog.pg_get_userbyid(datdba) AS \"Owner\", \
                   pg_catalog.pg_encoding_to_char(encoding) AS \"Encoding\" \
                   FROM pg_catalog.pg_database ORDER BY 1;";
    let result = QueryExecutor::execute(pool.client(), sql).await?;
    let opts = build_format_opts(args);
    let output = format_result(&result, &OutputFormat::Table, &opts)?;
    write_output(&output, args)?;
    Ok(())
}

/// `--command`: runs one SQL statement or meta-command, then exits.
pub async fn run_command(
    args: &CliArgs,
    conn_cfg: &ConnectionConfig,
    sql: &str,
) -> pgcli_core::Result<()> {
    let pool = ConnectionPool::connect(conn_cfg).await?;
    let format = args.format.parse::<OutputFormat>().unwrap_or_default();
    let opts = build_format_opts(args);
    let pager = build_pager(args);
    let repeat = args.repeat.max(1);
    let show_stats = args.stats || repeat > 1;
    let trimmed = sql.trim();

    let mut dispatcher = MetaCommandDispatcher::new();
    seed_variables(&mut dispatcher, args);

    if trimmed.starts_with('\\') {
        if let Some(cmd) = MetaCommand::parse(trimmed) {
            match dispatcher.dispatch(&cmd)? {
                MetaResult::Output(s) => write_output(&s, args)?,
                MetaResult::Query(q) => {
                    let result = QueryExecutor::execute(pool.client(), &q).await?;
                    let out = format_result(&result, &format, &opts)?;
                    write_output_or_pager(&out, args, &pager)?;
                }
                MetaResult::IntrospectTable { schema, name } => {
                    let intr = Introspector::new(pool.client());
                    let desc = intr.describe_table(&schema, &name).await?;
                    let out = format_result(&desc.raw, &format, &opts)?;
                    write_output_or_pager(&out, args, &pager)?;
                }
                MetaResult::DdlTable { schema, name } => {
                    let intr = Introspector::new(pool.client());
                    let ddl = intr.generate_ddl(ObjectType::Table, &schema, &name).await?;
                    write_output(&ddl, args)?;
                }
                MetaResult::ShowFunctionSource { schema, name } => {
                    let intr = Introspector::new(pool.client());
                    match intr.show_function_source(&schema, &name).await {
                        Ok(src) => write_output(&src, args)?,
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
                MetaResult::ShowViewDefinition { schema, name } => {
                    let intr = Introspector::new(pool.client());
                    match intr.show_view_definition(&schema, &name).await {
                        Ok(def) => write_output(&def, args)?,
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
                MetaResult::IntrospectTableExtended { schema, name } => {
                    let intr = Introspector::new(pool.client());
                    // First show normal column description.
                    match intr.describe_table(&schema, &name).await {
                        Ok(desc) => {
                            let base = format_result(&desc.raw, &format, &opts)?;
                            write_output_or_pager(&base, args, &pager)?;
                        }
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                    // Then append extended info (constraints, FKs, triggers).
                    match intr.describe_table_extended(&schema, &name).await {
                        Ok(ext) => write_output(&ext, args)?,
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
                MetaResult::ExecuteFile(path) => {
                    let mut runner = ScriptRunner::new();
                    runner.single_step = args.single_step;
                    seed_variables_into_runner(&mut runner, args);
                    let results = runner
                        .run_file(pool.client(), &path, args.single_transaction)
                        .await?;
                    let mut combined = String::new();
                    for result in results {
                        combined.push_str(&format_result(&result, &format, &opts)?);
                        combined.push('\n');
                    }
                    write_output_or_pager(&combined, args, &pager)?;
                }
                _ => {}
            }
        }
    } else {
        // Apply variable substitution before executing SQL.
        let sql = &dispatcher.substitute_vars(sql);
        let mut bench = BenchStats::new(repeat);
        let mut last_output = String::new();

        for i in 0..repeat {
            let t0 = std::time::Instant::now();
            let results = QueryExecutor::execute_batch(pool.client(), sql).await?;
            let elapsed = t0.elapsed();
            let mut run_rows = 0usize;
            let mut run_bytes = 0usize;
            let mut run_output = String::new();
            for result in &results {
                run_rows += result.rows.len();
                let out = if result.columns.is_empty() {
                    let tag = &result.command_tag;
                    let timing = if opts.timing {
                        format!(
                            " - {}",
                            pgcli_output::formats::format_duration(result.duration_ms)
                        )
                    } else {
                        String::new()
                    };
                    format!("{tag}{timing}\n")
                } else if result.command_tag == "EXPLAIN"
                    && format == pgcli_output::formats::OutputFormat::Table
                {
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
                    let table_out = format_result(result, &format, &opts)?;
                    // For DML with RETURNING, also emit the command tag (psql compat).
                    let verb = result.command_tag.split_whitespace().next().unwrap_or("");
                    if matches!(verb, "INSERT" | "UPDATE" | "DELETE") {
                        format!("{}{}\n", table_out, result.command_tag)
                    } else {
                        table_out
                    }
                };
                run_bytes += estimate_result_bytes(&out);
                run_output.push_str(&out);
            }
            bench.record(elapsed, run_rows, run_bytes);
            if i == 0 {
                last_output = run_output;
            }
        }

        write_output_or_pager(&last_output, args, &pager)?;
        if show_stats {
            eprint!("{}", bench.report());
        }

        // --export: write last result to file with format from extension.
        if let Some(ref export_path) = args.export {
            export_to_file(pool.client(), sql, export_path).await?;
        }
    }
    Ok(())
}

/// `--file`: runs a SQL script, then exits.
pub async fn run_file(
    args: &CliArgs,
    conn_cfg: &ConnectionConfig,
    path: &Path,
) -> pgcli_core::Result<()> {
    let pool = ConnectionPool::connect(conn_cfg).await?;
    let format = args.format.parse::<OutputFormat>().unwrap_or_default();
    let opts = build_format_opts(args);
    let mut runner = ScriptRunner::new();
    runner.single_step = args.single_step;
    // Seed script runner variables from -v flags.
    for pair in &args.set {
        if let Some((k, v)) = pair.split_once('=') {
            runner.set_variable(k, v);
        }
    }
    let results = runner
        .run_file(pool.client(), path, args.single_transaction)
        .await?;
    let pager = build_pager(args);
    for result in results {
        let output = if result.columns.is_empty() {
            format!("{}\n", result.command_tag)
        } else {
            format_result(&result, &format, &opts)?
        };
        write_output_or_pager(&output, args, &pager)?;
    }
    Ok(())
}
