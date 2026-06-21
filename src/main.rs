/// pgcli-rs entry point.
///
/// Parses CLI arguments, initializes tracing, builds the connection config,
/// and dispatches to interactive REPL or non-interactive execution modes.
use std::io::Write as IoWrite;
use std::process;
use std::time::Duration;

use clap::Parser;
use tracing::{error, info};
use tracing_subscriber::{fmt, EnvFilter};

use pgcli_rs::{
    cli::CliArgs,
    connection::{config::ConnectionConfig, pool::ConnectionPool},
    error::PgCliError,
    executor::query::QueryExecutor,
    meta::{
        commands::{MetaCommand, MetaCommandDispatcher, MetaResult},
        introspection::{Introspector, ObjectType},
    },
    output::{
        formats::{format_result, FormatOptions, OutputFormat},
        pager::Pager,
        stats::{estimate_result_bytes, BenchStats},
    },
    repl::{editor::ReplEditor, highlighter::SqlHighlighter, schema_cache::SchemaCache},
    scripting::runner::ScriptRunner,
};

#[tokio::main]
async fn main() {
    let args = CliArgs::parse();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if args.quiet {
            EnvFilter::new("error")
        } else {
            EnvFilter::new("warn")
        }
    });
    fmt().with_env_filter(filter).init();

    if let Err(e) = run(args).await {
        if matches!(e, PgCliError::Interrupted) {
            process::exit(0);
        }
        error!("{e}");
        eprintln!("Error: {e}");
        process::exit(1);
    }
}

async fn run(args: CliArgs) -> pgcli_rs::Result<()> {
    let mut conn_cfg = ConnectionConfig::from_cli_args(&args)?;

    if conn_cfg.password.is_none() && !args.no_password && args.password {
        conn_cfg.password = Some(prompt_password(&format!(
            "Password for user {}: ",
            conn_cfg.user
        ))?);
    }

    // --list: print databases and exit.
    if args.list {
        let pool = ConnectionPool::connect(&conn_cfg).await?;
        let sql = "SELECT datname AS \"Name\", \
                   pg_catalog.pg_get_userbyid(datdba) AS \"Owner\", \
                   pg_catalog.pg_encoding_to_char(encoding) AS \"Encoding\" \
                   FROM pg_catalog.pg_database ORDER BY 1;";
        let result = QueryExecutor::execute(pool.client(), sql).await?;
        let opts = build_format_opts(&args);
        let output = format_result(&result, &OutputFormat::Table, &opts)?;
        write_output(&output, &args)?;
        return Ok(());
    }

    // --command: single SQL or meta-command then exit.
    if let Some(ref sql) = args.command {
        let pool = ConnectionPool::connect(&conn_cfg).await?;
        let format = args.format.parse::<OutputFormat>().unwrap_or_default();
        let opts = build_format_opts(&args);
        let pager = build_pager(&args);
        let repeat = args.repeat.max(1);
        let show_stats = args.stats || repeat > 1;
        let trimmed = sql.trim();

        let mut dispatcher = MetaCommandDispatcher::new();
        seed_variables(&mut dispatcher, &args);

        if trimmed.starts_with('\\') {
            if let Some(cmd) = MetaCommand::parse(trimmed) {
                match dispatcher.dispatch(&cmd)? {
                    MetaResult::Output(s) => write_output(&s, &args)?,
                    MetaResult::Query(q) => {
                        let result = QueryExecutor::execute(pool.client(), &q).await?;
                        let out = format_result(&result, &format, &opts)?;
                        write_output_or_pager(&out, &args, &pager)?;
                    }
                    MetaResult::IntrospectTable { schema, name } => {
                        let intr = Introspector::new(pool.client());
                        let desc = intr.describe_table(&schema, &name).await?;
                        let out = format_result(&desc.raw, &format, &opts)?;
                        write_output_or_pager(&out, &args, &pager)?;
                    }
                    MetaResult::DdlTable { schema, name } => {
                        let intr = Introspector::new(pool.client());
                        let ddl = intr.generate_ddl(ObjectType::Table, &schema, &name).await?;
                        write_output(&ddl, &args)?;
                    }
                    MetaResult::ShowFunctionSource { schema, name } => {
                        let intr = Introspector::new(pool.client());
                        match intr.show_function_source(&schema, &name).await {
                            Ok(src) => write_output(&src, &args)?,
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    MetaResult::ShowViewDefinition { schema, name } => {
                        let intr = Introspector::new(pool.client());
                        match intr.show_view_definition(&schema, &name).await {
                            Ok(def) => write_output(&def, &args)?,
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    MetaResult::IntrospectTableExtended { schema, name } => {
                        let intr = Introspector::new(pool.client());
                        // First show normal column description.
                        match intr.describe_table(&schema, &name).await {
                            Ok(desc) => {
                                let base = format_result(&desc.raw, &format, &opts)?;
                                write_output_or_pager(&base, &args, &pager)?;
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                        // Then append extended info (constraints, FKs, triggers).
                        match intr.describe_table_extended(&schema, &name).await {
                            Ok(ext) => write_output(&ext, &args)?,
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    MetaResult::ExecuteFile(path) => {
                        let mut runner = ScriptRunner::new();
                        runner.single_step = args.single_step;
                        seed_variables_into_runner(&mut runner, &args);
                        let results = runner
                            .run_file(pool.client(), &path, args.single_transaction)
                            .await?;
                        let mut combined = String::new();
                        for result in results {
                            combined.push_str(&format_result(&result, &format, &opts)?);
                            combined.push('\n');
                        }
                        write_output_or_pager(&combined, &args, &pager)?;
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
                                pgcli_rs::output::formats::format_duration(result.duration_ms)
                            )
                        } else {
                            String::new()
                        };
                        format!("{tag}{timing}\n")
                    } else if result.command_tag == "EXPLAIN"
                        && format == pgcli_rs::output::formats::OutputFormat::Table
                    {
                        let plan_lines: Vec<String> = result
                            .rows
                            .iter()
                            .map(|r| r.values.first().map(|v| v.to_string()).unwrap_or_default())
                            .collect();
                        pgcli_rs::output::formats::colorize_explain_plan(
                            &plan_lines,
                            atty::is(atty::Stream::Stdout),
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

            write_output_or_pager(&last_output, &args, &pager)?;
            if show_stats {
                eprint!("{}", bench.report());
            }

            // --export: write last result to file with format from extension.
            if let Some(ref export_path) = args.export {
                export_to_file(pool.client(), sql, export_path).await?;
            }
        }
        return Ok(());
    }

    // --file: execute a SQL script and exit.
    if let Some(ref path) = args.file {
        let pool = ConnectionPool::connect(&conn_cfg).await?;
        let format = args.format.parse::<OutputFormat>().unwrap_or_default();
        let opts = build_format_opts(&args);
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
        let pager = build_pager(&args);
        for result in results {
            let output = if result.columns.is_empty() {
                format!("{}\n", result.command_tag)
            } else {
                format_result(&result, &format, &opts)?
            };
            write_output_or_pager(&output, &args, &pager)?;
        }
        return Ok(());
    }

    // Interactive REPL mode.
    info!("Starting interactive REPL");
    let mut pool = ConnectionPool::connect(&conn_cfg).await?;
    let dbname = conn_cfg.database.clone();

    // Build schema cache and do an initial refresh in the background.
    let schema_cache = SchemaCache::new_shared();
    if let Err(e) = SchemaCache::refresh(&schema_cache, pool.client()).await {
        tracing::warn!("Schema cache initial load failed: {e}");
    }

    let highlighter = SqlHighlighter::new(&args.theme).with_cache(schema_cache.clone());
    let history_path = args
        .history_file
        .clone()
        .or_else(|| dirs::home_dir().map(|h| h.join(".pgcli_history")));

    let mut editor = ReplEditor::new(
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
    seed_variables(&mut dispatcher, &args);

    let mut format = args.format.parse::<OutputFormat>().unwrap_or_default();
    let mut opts = build_format_opts(&args);
    let mut pager = build_pager(&args);

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

    loop {
        let input = match editor.readline() {
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
                match dispatcher.dispatch(&cmd) {
                    Ok(MetaResult::Quit) => {
                        println!("Bye.");
                        break;
                    }
                    Ok(MetaResult::Output(s)) => println!("{s}"),
                    Ok(MetaResult::ChangeTheme(t)) => {
                        opts.theme = t.clone();
                        println!("Theme set to '{t}'. SQL highlighting updates on next input.");
                    }
                    Ok(MetaResult::Reconnect {
                        dbname,
                        user,
                        host,
                        port,
                    }) => {
                        let db_display = dbname
                            .clone()
                            .unwrap_or_else(|| pool.config().database.clone());
                        match pool.reconnect_to(dbname, user, host, port).await {
                            Ok(()) => {
                                println!(
                                    "You are now connected to database \"{}\" as user \"{}\".",
                                    pool.config().database,
                                    pool.config().user
                                );
                                // Refresh schema cache for the new database.
                                if let Err(e) =
                                    SchemaCache::refresh(&schema_cache, pool.client()).await
                                {
                                    tracing::warn!(
                                        "Schema cache refresh after reconnect failed: {e}"
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("ERROR: could not connect to \"{db_display}\": {e}")
                            }
                        }
                        // Reset last_sql so \watch doesn't rerun the previous db's query.
                        dispatcher.last_sql.clear();
                    }
                    Ok(MetaResult::ExecuteFile(path)) => {
                        let mut runner = ScriptRunner::new();
                        for (k, v) in &dispatcher.variables {
                            runner.set_variable(k, v);
                        }
                        match runner.run_file(pool.client(), &path, false).await {
                            Ok(results) => {
                                for result in results {
                                    opts.expanded = dispatcher.expanded;
                                    opts.timing = dispatcher.timing;
                                    let out =
                                        format_result(&result, &format, &opts).unwrap_or_default();
                                    pager.print(&out).ok();
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ExecuteFileVerbose(path)) => {
                        let content = match std::fs::read_to_string(&path) {
                            Ok(s) => s,
                            Err(e) => {
                                eprintln!("ERROR: {e}");
                                continue;
                            }
                        };
                        let mut runner = ScriptRunner::new();
                        for (k, v) in &dispatcher.variables {
                            runner.set_variable(k, v);
                        }
                        for stmt in content.split(';') {
                            let s = stmt.trim();
                            if s.is_empty() || s.starts_with("--") {
                                continue;
                            }
                            println!("-- {s}");
                            match QueryExecutor::execute_batch(pool.client(), &format!("{s};"))
                                .await
                            {
                                Ok(results) => {
                                    for result in results {
                                        opts.expanded = dispatcher.expanded;
                                        opts.timing = dispatcher.timing;
                                        let out = format_result(&result, &format, &opts)
                                            .unwrap_or_default();
                                        pager.print(&out).ok();
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                        }
                    }
                    Ok(MetaResult::WriteResult(path)) => {
                        if dispatcher.last_sql.is_empty() {
                            eprintln!("\\write: no previous query to write.");
                        } else {
                            match QueryExecutor::execute_batch(
                                pool.client(),
                                &dispatcher.last_sql.clone(),
                            )
                            .await
                            {
                                Ok(results) => {
                                    let mut combined = String::new();
                                    for result in &results {
                                        opts.expanded = dispatcher.expanded;
                                        opts.timing = false;
                                        combined.push_str(
                                            &format_result(result, &format, &opts)
                                                .unwrap_or_default(),
                                        );
                                        combined.push('\n');
                                    }
                                    match std::fs::write(&path, &combined) {
                                        Ok(_) => println!("Result written to '{path}'."),
                                        Err(e) => eprintln!("\\write: {e}"),
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                        }
                    }
                    Ok(MetaResult::Watch { interval_secs }) => {
                        let sql = dispatcher.last_sql.clone();
                        println!("Watching every {interval_secs}s - press Ctrl-C to stop.\n");
                        loop {
                            match QueryExecutor::execute_batch(pool.client(), &sql).await {
                                Ok(results) => {
                                    // Clear screen for watch mode.
                                    print!("\x1b[2J\x1b[H");
                                    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                                    println!("-- {sql}  [{now}] (every {interval_secs}s)");
                                    for result in &results {
                                        opts.expanded = effective_expanded(&dispatcher, result);
                                        opts.timing = dispatcher.timing;
                                        let out = format_result(result, &format, &opts)
                                            .unwrap_or_default();
                                        println!("{out}");
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
                        }
                    }
                    Ok(MetaResult::IntrospectTable { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.describe_table(&schema, &name).await {
                            Ok(desc) => {
                                opts.expanded = dispatcher.expanded;
                                opts.timing = false;
                                let out =
                                    format_result(&desc.raw, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                                // Append constraint section (PK, UNIQUE, CHECK).
                                match introspector
                                    .describe_table_constraints(&schema, &name)
                                    .await
                                {
                                    Ok(cs) if !cs.is_empty() => {
                                        pager.print(&cs).ok();
                                    }
                                    _ => {}
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::DdlTable { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector
                            .generate_ddl(ObjectType::Table, &schema, &name)
                            .await
                        {
                            Ok(ddl) => {
                                pager.print(&ddl).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShowFunctionSource { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_function_source(&schema, &name).await {
                            Ok(src) => {
                                pager.print(&src).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShowViewDefinition { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_view_definition(&schema, &name).await {
                            Ok(def) => {
                                pager.print(&def).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::IntrospectTableExtended { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        // Base column listing.
                        match introspector.describe_table(&schema, &name).await {
                            Ok(desc) => {
                                opts.expanded = dispatcher.expanded;
                                opts.timing = false;
                                let out =
                                    format_result(&desc.raw, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                        // Extended sections: constraints, FKs, triggers.
                        match introspector.describe_table_extended(&schema, &name).await {
                            Ok(ext) => {
                                pager.print(&ext).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::SetPrintOption { key, value }) => match key.as_str() {
                        "prompt1" => {
                            editor.prompt1 = value.filter(|v| !v.is_empty());
                            println!("PROMPT1 updated.");
                        }
                        "prompt2" => {
                            editor.prompt2 = value.filter(|v| !v.is_empty());
                            println!("PROMPT2 updated.");
                        }
                        _ => {
                            apply_pset(&key, value, &mut format, &mut opts, &mut dispatcher);
                            pager.disabled = !opts.pager_enabled;
                        }
                    },
                    Ok(MetaResult::GSet { prefix }) => {
                        let sql = dispatcher.last_sql.clone();
                        match QueryExecutor::execute(pool.client(), &sql).await {
                            Ok(result) => {
                                if let Some(row) = result.rows.first() {
                                    let count = result.columns.len();
                                    for (col, val) in result.columns.iter().zip(row.values.iter()) {
                                        let key = format!("{}{}", prefix, col.name);
                                        let value = val.to_string();
                                        dispatcher.variables.insert(key, value);
                                    }
                                    println!("Stored {count} variable(s).");
                                } else {
                                    eprintln!("\\gset: query returned no rows.");
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::CrossTabView {
                        col_h,
                        col_v,
                        col_d,
                    }) => {
                        let sql = dispatcher.last_sql.clone();
                        if sql.is_empty() {
                            eprintln!("\\crosstabview: no previous query.");
                        } else {
                            match QueryExecutor::execute(pool.client(), &sql).await {
                                Ok(result) => {
                                    match crosstab_pivot(&result, &col_h, &col_v, &col_d) {
                                        Ok(out) => {
                                            pager.print(&out).ok();
                                        }
                                        Err(e) => eprintln!("\\crosstabview: {e}"),
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                        }
                    }
                    Ok(MetaResult::GDesc) => {
                        let sql =
                            format!("SELECT * FROM ({}) __pgcli_q LIMIT 0", dispatcher.last_sql);
                        match QueryExecutor::execute(pool.client(), &sql).await {
                            Ok(result) => {
                                let header = "Column                          Type";
                                let sep = "-".repeat(header.len());
                                println!("{header}\n{sep}");
                                for col in &result.columns {
                                    println!("{:<32}{}", col.name, col.type_name);
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::Bench { count }) => {
                        let sql = dispatcher.last_sql.clone();
                        println!(
                            "Benchmarking {count} run(s): {}{}",
                            sql.chars().take(60).collect::<String>(),
                            if sql.chars().count() > 60 { "..." } else { "" }
                        );
                        let mut bench = BenchStats::new(count);
                        let mut total_rows = 0usize;
                        for _ in 0..count {
                            let t0 = std::time::Instant::now();
                            match QueryExecutor::execute(pool.client(), &sql).await {
                                Ok(result) => {
                                    let elapsed = t0.elapsed();
                                    let rows = result.rows.len();
                                    total_rows += rows;
                                    bench.record(elapsed, rows, 0);
                                }
                                Err(e) => {
                                    eprintln!("ERROR: {e}");
                                    break;
                                }
                            }
                        }
                        let _ = total_rows;
                        eprint!("{}", bench.report());
                    }
                    Ok(MetaResult::EditAndExecute) => {
                        let last = dispatcher.last_sql.clone();
                        let new_sql = tokio::task::spawn_blocking(move || {
                            let tmp = std::env::temp_dir().join("pgcli_edit.sql");
                            std::fs::write(&tmp, &last).ok();
                            let editor = std::env::var("VISUAL")
                                .or_else(|_| std::env::var("EDITOR"))
                                .unwrap_or_else(|_| {
                                    if cfg!(windows) {
                                        "notepad.exe".to_string()
                                    } else {
                                        "vi".to_string()
                                    }
                                });
                            #[cfg(windows)]
                            let _ = std::process::Command::new("cmd")
                                .args(["/C", "start", "/wait", "", &editor])
                                .arg(&tmp)
                                .status();
                            #[cfg(not(windows))]
                            let _ = std::process::Command::new(&editor).arg(&tmp).status();
                            std::fs::read_to_string(&tmp).unwrap_or_default()
                        })
                        .await
                        .unwrap_or_default();

                        let new_sql = new_sql.trim().to_string();
                        if new_sql.is_empty() {
                            println!("(empty buffer - nothing executed)");
                        } else {
                            dispatcher.last_sql = new_sql.clone();
                            match QueryExecutor::execute_batch(pool.client(), &new_sql).await {
                                Ok(results) => {
                                    for result in results {
                                        opts.expanded = dispatcher.expanded;
                                        opts.timing = dispatcher.timing;
                                        let out = format_result(&result, &format, &opts)
                                            .unwrap_or_default();
                                        pager.print(&out).ok();
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                        }
                    }
                    Ok(MetaResult::Query(sql)) => {
                        match QueryExecutor::execute(pool.client(), &sql).await {
                            Ok(result) => {
                                opts.expanded = effective_expanded(&dispatcher, &result);
                                opts.timing = dispatcher.timing;
                                if args.echo_hidden {
                                    eprintln!(">> {sql}");
                                }
                                let out = if result.columns.is_empty() {
                                    let tag = &result.command_tag;
                                    let timing = if opts.timing {
                                        format!(
                                            " - {}",
                                            pgcli_rs::output::formats::format_duration(
                                                result.duration_ms
                                            )
                                        )
                                    } else {
                                        String::new()
                                    };
                                    format!("{tag}{timing}")
                                } else {
                                    format_result(&result, &format, &opts).unwrap_or_default()
                                };
                                pager.print(&out).ok();
                            }
                            Err(e) => {
                                eprintln!("ERROR: {e}");
                                if args.echo_errors {
                                    eprintln!("Failed command: {sql}");
                                }
                            }
                        }
                    }
                    Ok(MetaResult::Sleep(secs)) => {
                        if secs > 0.0 {
                            tokio::time::sleep(std::time::Duration::from_secs_f64(secs)).await;
                        }
                    }
                    Ok(MetaResult::LoImport(path)) => match std::fs::read(&path) {
                        Ok(data) => {
                            let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
                            let sql = format!("SELECT lo_from_bytea(0, '\\x{hex}'::bytea) AS oid");
                            match QueryExecutor::execute(pool.client(), &sql).await {
                                Ok(res) => {
                                    let oid_str = res
                                        .rows
                                        .first()
                                        .and_then(|r| r.values.first())
                                        .map(|v| format!("{v}"))
                                        .unwrap_or_default();
                                    println!("lo_import {oid_str}");
                                }
                                Err(e) => tracing::error!("{e}"),
                            }
                        }
                        Err(e) => tracing::error!("lo_import: {e}"),
                    },
                    Ok(MetaResult::LoExport { oid, path }) => {
                        let sql = format!("SELECT lo_get({oid})");
                        match QueryExecutor::execute(pool.client(), &sql).await {
                            Ok(res) => {
                                use pgcli_rs::protocol::messages::CellValue;
                                let bytes: Vec<u8> = res
                                    .rows
                                    .first()
                                    .and_then(|r| r.values.first())
                                    .and_then(|v| {
                                        if let CellValue::Bytea(b) = v {
                                            Some(b.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .unwrap_or_default();
                                if let Err(e) = std::fs::write(&path, &bytes) {
                                    tracing::error!("lo_export: {e}");
                                } else {
                                    println!("lo_export {oid}");
                                }
                            }
                            Err(e) => tracing::error!("{e}"),
                        }
                    }
                    Ok(MetaResult::OutputNoNl(s)) => print!("{s}"),
                    Ok(MetaResult::Repeat { to_file }) => {
                        let sql = dispatcher.last_sql.clone();
                        if sql.is_empty() {
                            eprintln!("\\g: no previous query.");
                        } else {
                            match QueryExecutor::execute_batch(pool.client(), &sql).await {
                                Ok(results) => {
                                    for result in results {
                                        opts.expanded = effective_expanded(&dispatcher, &result);
                                        opts.timing = dispatcher.timing;
                                        let out = if result.columns.is_empty() {
                                            result.command_tag.clone()
                                        } else {
                                            format_result(&result, &format, &opts)
                                                .unwrap_or_default()
                                        };
                                        if let Some(ref path) = to_file {
                                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                                .create(true)
                                                .append(true)
                                                .open(path)
                                            {
                                                writeln!(f, "{out}").ok();
                                            }
                                        } else {
                                            pager.print(&out).ok();
                                        }
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                        }
                    }
                    Ok(MetaResult::RepeatExpanded) => {
                        let sql = dispatcher.last_sql.clone();
                        if sql.is_empty() {
                            eprintln!("\\gx: no previous query.");
                        } else {
                            match QueryExecutor::execute_batch(pool.client(), &sql).await {
                                Ok(results) => {
                                    for result in results {
                                        opts.expanded = true;
                                        opts.timing = dispatcher.timing;
                                        let out = format_result(&result, &format, &opts)
                                            .unwrap_or_default();
                                        pager.print(&out).ok();
                                    }
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                        }
                    }
                    Ok(MetaResult::GExec) => {
                        let sql = dispatcher.last_sql.clone();
                        match QueryExecutor::execute(pool.client(), &sql).await {
                            Ok(result) => {
                                let mut executed = 0usize;
                                for row in &result.rows {
                                    for cell in &row.values {
                                        let cell_sql = cell.to_string();
                                        if cell_sql.is_empty() {
                                            continue;
                                        }
                                        match QueryExecutor::execute(pool.client(), &cell_sql).await
                                        {
                                            Ok(_) => {
                                                executed += 1;
                                                println!(
                                                    "OK: {}",
                                                    &cell_sql[..cell_sql.len().min(80)]
                                                );
                                            }
                                            Err(e) => eprintln!(
                                                "ERROR: {e} (sql: {})",
                                                &cell_sql[..cell_sql.len().min(80)]
                                            ),
                                        }
                                    }
                                }
                                println!("\\gexec: {executed} statement(s) executed.");
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ClientCopy(spec)) => {
                        match client_copy(pool.client(), &spec).await {
                            Ok(msg) => println!("{msg}"),
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShowDeps { name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_deps(&name).await {
                            Ok(out) => pager.print(&out).ok(),
                            Err(e) => {
                                eprintln!("ERROR: {e}");
                                None
                            }
                        };
                    }
                    Ok(MetaResult::WatchDiff { interval_secs }) => {
                        let sql = dispatcher.last_sql.clone();
                        println!("Watching (diff) every {interval_secs}s — Ctrl-C to stop.\n");
                        let mut prev_lines: Vec<String> = Vec::new();
                        loop {
                            match QueryExecutor::execute_batch(pool.client(), &sql).await {
                                Ok(results) => {
                                    let mut cur_lines: Vec<String> = Vec::new();
                                    for result in &results {
                                        let out = format_result(result, &format, &opts)
                                            .unwrap_or_default();
                                        for l in out.lines() {
                                            cur_lines.push(l.to_string());
                                        }
                                    }
                                    print!("\x1b[2J\x1b[H");
                                    println!("-- {sql}  (diff every {interval_secs}s)");
                                    let prev_set: std::collections::HashSet<&String> =
                                        prev_lines.iter().collect();
                                    let cur_set: std::collections::HashSet<&String> =
                                        cur_lines.iter().collect();
                                    for l in &prev_lines {
                                        if !cur_set.contains(l) {
                                            println!("\x1b[31m- {l}\x1b[0m");
                                        }
                                    }
                                    for l in &cur_lines {
                                        if !prev_set.contains(l) {
                                            println!("\x1b[32m+ {l}\x1b[0m");
                                        }
                                    }
                                    if prev_lines == cur_lines && !cur_lines.is_empty() {
                                        println!("(no change)");
                                    }
                                    prev_lines = cur_lines;
                                }
                                Err(e) => eprintln!("ERROR: {e}"),
                            }
                            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
                        }
                    }
                    Ok(MetaResult::ShowIndexes { name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_indexes(&name).await {
                            Ok(result) => {
                                opts.expanded = false;
                                opts.timing = false;
                                let out =
                                    format_result(&result, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShowBloat) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_bloat().await {
                            Ok(result) => {
                                opts.expanded = false;
                                opts.timing = false;
                                let out =
                                    format_result(&result, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShowColumnStats { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_column_stats(&schema, &name).await {
                            Ok(result) => {
                                if result.rows.is_empty() {
                                    eprintln!(
                                        "No statistics for {schema}.{name} (run ANALYZE first)."
                                    );
                                } else {
                                    opts.expanded = false;
                                    opts.timing = false;
                                    let out =
                                        format_result(&result, &format, &opts).unwrap_or_default();
                                    pager.print(&out).ok();
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShowPartitions { schema, name }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.show_partitions(&schema, &name).await {
                            Ok(result) => {
                                if result.rows.is_empty() {
                                    println!("{schema}.{name} is not a partitioned table or has no partitions.");
                                } else {
                                    opts.expanded = false;
                                    opts.timing = false;
                                    let out =
                                        format_result(&result, &format, &opts).unwrap_or_default();
                                    pager.print(&out).ok();
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ShellExec(cmd_str)) => {
                        let shell_out = run_shell(&cmd_str);
                        print!("{shell_out}");
                    }
                    Ok(MetaResult::Warn(msg)) => {
                        eprintln!("WARNING: {msg}");
                    }
                    Ok(MetaResult::SetOnError(mode)) => {
                        dispatcher.on_error_mode = mode.clone();
                        println!("On error mode set to '{mode}'.");
                    }
                    Ok(MetaResult::Prompt { text, var }) => {
                        let prompt_text = if text.is_empty() {
                            format!("Enter value for {var}: ")
                        } else {
                            format!("{text} ")
                        };
                        eprint!("{prompt_text}");
                        use std::io::{BufRead, Write};
                        std::io::stderr().flush().ok();
                        let mut input = String::new();
                        std::io::stdin().lock().read_line(&mut input).ok();
                        let val = input
                            .trim_end_matches('\n')
                            .trim_end_matches('\r')
                            .to_string();
                        dispatcher.variables.insert(var, val);
                    }
                    Ok(MetaResult::ListRoles { pattern }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.list_roles(&pattern).await {
                            Ok(result) => {
                                opts.expanded = false;
                                opts.timing = false;
                                let out =
                                    format_result(&result, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ListSequences { pattern }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.list_sequences(&pattern).await {
                            Ok(result) => {
                                opts.expanded = false;
                                opts.timing = false;
                                let out =
                                    format_result(&result, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::KillBackend { pid, force }) => {
                        let fn_name = if force {
                            "pg_terminate_backend"
                        } else {
                            "pg_cancel_backend"
                        };
                        let sql = format!("SELECT {fn_name}({pid})");
                        match QueryExecutor::execute(pool.client(), &sql).await {
                            Ok(result) => {
                                let ok = result
                                    .rows
                                    .first()
                                    .and_then(|r| r.values.first())
                                    .map(|v| v.to_string() == "t")
                                    .unwrap_or(false);
                                if ok {
                                    println!(
                                        "Backend {pid} {}.",
                                        if force { "terminated" } else { "cancelled" }
                                    );
                                } else {
                                    eprintln!("Backend {pid} not found or permission denied.");
                                }
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::ListLanguages { pattern }) => {
                        let introspector = Introspector::new(pool.client());
                        match introspector.list_languages(&pattern).await {
                            Ok(result) => {
                                opts.expanded = false;
                                opts.timing = false;
                                let out =
                                    format_result(&result, &format, &opts).unwrap_or_default();
                                pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("ERROR: {e}"),
                        }
                    }
                    Ok(MetaResult::Ok) => {}
                    Err(e) => eprintln!("{e}"),
                }
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        // Interactive COPY FROM STDIN: read lines until \. then send via copy_in.
        let upper = trimmed.to_ascii_uppercase();
        if upper.contains("COPY") && upper.contains("FROM STDIN") {
            use futures_util::SinkExt;
            println!("Enter data (one row per line, end with \\.):");
            let mut copy_data = Vec::<u8>::new();
            let mut row_count: u64 = 0;
            while let Ok(Some(line)) = editor.readline() {
                if line.trim() == "\\." {
                    break;
                }
                copy_data.extend_from_slice(line.as_bytes());
                copy_data.push(b'\n');
                row_count += 1;
            }
            match pool.client().copy_in(trimmed).await {
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
            continue;
        }

        if args.echo_queries {
            println!(">> {trimmed}");
        }

        // Apply variable substitution before execution.
        let sql = dispatcher.substitute_vars(trimmed);

        // Capture cancel token before borrowing pool for the query.
        let cancel_token = pool.client().cancel_token();

        // Show spinner on TTY for queries that may take time.
        let spinner = pgcli_rs::output::Spinner::start("running...");

        // Race query against Ctrl-C: on interrupt, send a cancel request to the server.
        let exec_result = tokio::select! {
            biased;
            r = QueryExecutor::execute_batch(pool.client(), &sql) => r,
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
            pool.client(),
            "SELECT CASE pg_current_xact_id_if_assigned() IS NOT NULL WHEN true THEN '*' ELSE '' END",
        ).await {
            if let Some(row) = txn_row.rows.first() {
                if let Some(val) = row.values.first() {
                    editor.txn_status = val.to_string();
                }
            }
        }

        match exec_result {
            Ok(results) => {
                // Remember this SQL for \watch, \p, \bench, \gset.
                dispatcher.last_sql = sql.trim_end_matches(';').trim().to_string();
                for result in results {
                    opts.expanded = effective_expanded(&dispatcher, &result);
                    opts.timing = dispatcher.timing;

                    // For zero-column results (VACUUM, ANALYZE, SET, etc.), show command tag.
                    let out = if result.columns.is_empty() {
                        let tag = &result.command_tag;
                        let timing = if opts.timing {
                            format!(
                                " - {}",
                                pgcli_rs::output::formats::format_duration(result.duration_ms)
                            )
                        } else {
                            String::new()
                        };
                        format!("{tag}{timing}")
                    } else if result.command_tag == "EXPLAIN" && format == OutputFormat::Table {
                        // EXPLAIN results: apply colorized plan renderer instead of table.
                        let plan_lines: Vec<String> = result
                            .rows
                            .iter()
                            .map(|r| r.values.first().map(|v| v.to_string()).unwrap_or_default())
                            .collect();
                        pgcli_rs::output::formats::colorize_explain_plan(
                            &plan_lines,
                            atty::is(atty::Stream::Stdout),
                        )
                    } else {
                        let table_out = format_result(&result, &format, &opts).unwrap_or_default();
                        // For DML with RETURNING, also emit the command tag (psql compat).
                        let verb = result.command_tag.split_whitespace().next().unwrap_or("");
                        if matches!(verb, "INSERT" | "UPDATE" | "DELETE") {
                            format!("{}{}", table_out, result.command_tag)
                        } else {
                            table_out
                        }
                    };

                    // \o file redirect.
                    if let Some(ref out_path) = dispatcher.output_file {
                        if let Ok(mut f) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(out_path)
                        {
                            writeln!(f, "{out}").ok();
                        }
                    } else {
                        pager.print(&out).ok();
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
                    if args.echo_errors {
                        eprintln!("Failed: {trimmed}");
                    }
                    // Auto-reconnect on connection loss.
                    if !pool.is_alive().await {
                        eprintln!("Connection to server was lost. Reconnecting...");
                        match pool.reconnect().await {
                            Ok(()) => {
                                eprintln!("Reconnected to \"{}\".", pool.config().database);
                                if let Err(ce) =
                                    SchemaCache::refresh(&schema_cache, pool.client()).await
                                {
                                    tracing::warn!("Schema cache refresh after reconnect: {ce}");
                                }
                            }
                            Err(re) => eprintln!("Reconnect failed: {re}"),
                        }
                    }
                }
            }
        }
    }

    editor.save_history().ok();
    Ok(())
}

/// Seed dispatcher variables from `-v NAME=VALUE` CLI flags.
fn seed_variables(dispatcher: &mut MetaCommandDispatcher, args: &CliArgs) {
    for pair in &args.set {
        if let Some((k, v)) = pair.split_once('=') {
            dispatcher.variables.insert(k.to_string(), v.to_string());
        }
    }
}

/// Seed a ScriptRunner's variables from `-v NAME=VALUE` CLI flags.
fn seed_variables_into_runner(runner: &mut ScriptRunner, args: &CliArgs) {
    for pair in &args.set {
        if let Some((k, v)) = pair.split_once('=') {
            runner.set_variable(k, v);
        }
    }
}

/// Build `FormatOptions` from CLI args.
fn build_format_opts(args: &CliArgs) -> FormatOptions {
    let format = if args.html {
        // -H flag sets HTML output; handled via OutputFormat, not opts.
        "table"
    } else if args.no_align {
        "unaligned"
    } else {
        "table"
    };
    let _ = format; // OutputFormat is set separately; opts only carries per-row options.
    FormatOptions {
        tuples_only: args.tuples_only,
        field_separator: args.field_separator.clone(),
        record_separator: args.record_separator.clone(),
        theme: args.theme.clone(),
        ..FormatOptions::default()
    }
}

/// Build a `Pager` from CLI args.
fn build_pager(args: &CliArgs) -> Pager {
    if args.no_pager {
        Pager::disabled()
    } else if let Some(ref cmd) = args.pager {
        Pager::with_command(cmd)
    } else {
        Pager::default()
    }
}

/// Determine the effective expanded mode considering `auto` expansion.
///
/// In auto mode, expands if the estimated result width exceeds the terminal width.
/// Pivot a `QueryResult` into a cross-tab (pivot table) string.
///
/// `col_h` is the column whose values become horizontal headers (columns).
/// `col_v` is the column whose values become the row key (left-most column).
/// `col_d` is the column whose values become cell data.
/// If names are empty, defaults are used: col 0, col 1, col 2.
fn crosstab_pivot(
    result: &pgcli_rs::protocol::messages::QueryResult,
    col_h: &str,
    col_v: &str,
    col_d: &str,
) -> std::result::Result<String, String> {
    if result.columns.len() < 3 {
        return Err("\\crosstabview requires at least 3 columns in the query result.".to_string());
    }

    let find_col = |name: &str, default: usize| -> usize {
        if name.is_empty() {
            default
        } else {
            result
                .columns
                .iter()
                .position(|c| c.name == name)
                .unwrap_or(default)
        }
    };

    let h_idx = find_col(col_h, 0);
    let v_idx = find_col(col_v, 1);
    let d_idx = find_col(col_d, 2);

    // Collect unique column headers (h values) and row keys (v values).
    use std::collections::{BTreeMap, BTreeSet};
    let mut headers: BTreeSet<String> = BTreeSet::new();
    let mut cells: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    for row in &result.rows {
        let h_val = row
            .values
            .get(h_idx)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let v_val = row
            .values
            .get(v_idx)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let d_val = row
            .values
            .get(d_idx)
            .map(|v| v.to_string())
            .unwrap_or_default();
        headers.insert(h_val.clone());
        cells.entry(v_val).or_default().insert(h_val, d_val);
    }

    let headers: Vec<String> = headers.into_iter().collect();

    // Build output table.
    let row_key_col = result
        .columns
        .get(v_idx)
        .map(|c| c.name.as_str())
        .unwrap_or("Row");
    let mut out = format!("| {row_key_col}");
    for h in &headers {
        out.push_str(&format!(" | {h}"));
    }
    out.push_str(" |\n| ---");
    for _ in &headers {
        out.push_str(" | ---");
    }
    out.push_str(" |\n");

    for (row_key, row_cells) in &cells {
        out.push_str(&format!("| {row_key}"));
        for h in &headers {
            let cell = row_cells.get(h).map(|s| s.as_str()).unwrap_or("");
            out.push_str(&format!(" | {cell}"));
        }
        out.push_str(" |\n");
    }

    Ok(out)
}

fn effective_expanded(
    dispatcher: &MetaCommandDispatcher,
    result: &pgcli_rs::protocol::messages::QueryResult,
) -> bool {
    if dispatcher.expanded {
        return true;
    }
    if !dispatcher.expanded_auto {
        return false;
    }
    let term_width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120);
    let estimated: usize = result.columns.iter().map(|c| c.name.len() + 14).sum();
    estimated > term_width
}

/// Execute a shell command and return its combined stdout+stderr as a string.
fn run_shell(cmd_str: &str) -> String {
    if cmd_str.trim().is_empty() {
        return String::new();
    }
    #[cfg(unix)]
    let result = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd_str)
        .output();
    #[cfg(windows)]
    let result = std::process::Command::new("cmd")
        .args(["/C", cmd_str])
        .output();
    match result {
        Ok(out) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.is_empty() {
                s.push_str(&err);
            }
            s
        }
        Err(e) => format!("ERROR: shell exec failed: {e}\n"),
    }
}

/// Load `~/.pgclirc` and apply `\set` and `\pset` directives found there.
///
/// Lines starting with `#` or empty lines are ignored. Only `\set` and `\pset`
/// meta-commands are honoured during startup to avoid side-effects.
fn load_startup_file(
    dispatcher: &mut MetaCommandDispatcher,
    format: &mut OutputFormat,
    opts: &mut FormatOptions,
) {
    let Some(home) = dirs::home_dir() else { return };
    let rc_path = home.join(".pgclirc");
    let Ok(content) = std::fs::read_to_string(&rc_path) else {
        return;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(cmd) = MetaCommand::parse(line) {
            // Only apply safe, side-effect-free directives at startup.
            if cmd.name == "set" || cmd.name == "pset" || cmd.name == "format" {
                if let Ok(MetaResult::SetPrintOption { key, value }) = dispatcher.dispatch(&cmd) {
                    apply_pset(&key, value, format, opts, dispatcher);
                }
            }
        }
    }
}

/// Write `output` to `--output FILE` if set, otherwise print to stdout.
fn write_output(output: &str, args: &CliArgs) -> pgcli_rs::Result<()> {
    if let Some(ref path) = args.output {
        std::fs::write(path, output).map_err(PgCliError::Io)?;
    } else {
        println!("{output}");
    }
    Ok(())
}

/// Write to pager, or to `--output FILE` if set.
fn write_output_or_pager(output: &str, args: &CliArgs, pager: &Pager) -> pgcli_rs::Result<()> {
    if let Some(ref path) = args.output {
        std::fs::write(path, output).map_err(PgCliError::Io)?;
    } else {
        pager.print(output).ok();
    }
    Ok(())
}

/// Export the result of `sql` to `path`, inferring format from the file extension.
async fn export_to_file(
    client: &tokio_postgres::Client,
    sql: &str,
    path: &std::path::Path,
) -> pgcli_rs::Result<()> {
    use pgcli_rs::export::{csv::CsvExporter, json::JsonExporter, sql::SqlExporter};

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let results = QueryExecutor::execute_batch(client, sql).await?;
    let mut file = std::fs::File::create(path).map_err(PgCliError::Io)?;

    for result in &results {
        match ext.as_str() {
            "csv" => CsvExporter::export(result, &mut file)?,
            "json" => JsonExporter::export_array(result, &mut file)?,
            "jsonl" | "ndjson" => JsonExporter::export_lines(result, &mut file)?,
            "tsv" => {
                let opts = FormatOptions::default();
                let out = format_result(result, &OutputFormat::Tsv, &opts)?;
                file.write_all(out.as_bytes()).map_err(PgCliError::Io)?;
            }
            "sql" => SqlExporter::export_insert(result, "exported_data", &mut file)?,
            _ => {
                // Default to CSV for unknown extensions.
                CsvExporter::export(result, &mut file)?;
            }
        }
    }
    println!("Exported to '{}'.", path.display());
    Ok(())
}

/// Execute a client-side COPY command, reading from or writing to a local file.
///
/// `spec` is the `\copy` argument string, e.g.:
/// - `"employees TO '/tmp/emp.csv' WITH (FORMAT CSV, HEADER true)"`
/// - `"employees FROM '/tmp/emp.csv' WITH (FORMAT CSV)"`
///
/// Transforms `TO 'file'` → `TO STDOUT` and `FROM 'file'` → `FROM STDIN`.
async fn client_copy(client: &tokio_postgres::Client, spec: &str) -> pgcli_rs::Result<String> {
    use futures_util::{SinkExt, TryStreamExt};
    use tokio::io::AsyncWriteExt;

    let spec_upper = spec.to_uppercase();

    if let Some(to_pos) = find_keyword_pos(&spec_upper, " TO ") {
        // Extract file path from  TO 'file' or TO file
        let after_to = spec[to_pos + 4..].trim_start();
        let (file_path, rest) = extract_path(after_to)?;

        let copy_sql = format!(
            "COPY {} TO STDOUT{}",
            &spec[..to_pos],
            if rest.is_empty() {
                String::new()
            } else {
                format!(" {rest}")
            }
        );

        let stream = client
            .copy_out(&copy_sql)
            .await
            .map_err(|e| pgcli_rs::error::PgCliError::Query(e.to_string()))?;

        let chunks: Vec<bytes::Bytes> = stream
            .try_collect()
            .await
            .map_err(|e| pgcli_rs::error::PgCliError::Query(e.to_string()))?;

        let mut f = tokio::fs::File::create(&file_path)
            .await
            .map_err(pgcli_rs::error::PgCliError::Io)?;
        let mut total = 0usize;
        for chunk in &chunks {
            f.write_all(chunk.as_ref())
                .await
                .map_err(pgcli_rs::error::PgCliError::Io)?;
            total += chunk.len();
        }
        Ok(format!("COPY: wrote {total} bytes to '{file_path}'"))
    } else if let Some(from_pos) = find_keyword_pos(&spec_upper, " FROM ") {
        let after_from = spec[from_pos + 6..].trim_start();
        let (file_path, rest) = extract_path(after_from)?;

        let copy_sql = format!(
            "COPY {} FROM STDIN{}",
            &spec[..from_pos],
            if rest.is_empty() {
                String::new()
            } else {
                format!(" {rest}")
            }
        );

        let data = std::fs::read(&file_path).map_err(pgcli_rs::error::PgCliError::Io)?;
        let row_count = data.iter().filter(|&&b| b == b'\n').count();

        let sink = client
            .copy_in(&copy_sql)
            .await
            .map_err(|e| pgcli_rs::error::PgCliError::Query(e.to_string()))?;
        futures_util::pin_mut!(sink);
        sink.send(bytes::Bytes::from(data))
            .await
            .map_err(|e| pgcli_rs::error::PgCliError::Query(e.to_string()))?;
        sink.close()
            .await
            .map_err(|e| pgcli_rs::error::PgCliError::Query(e.to_string()))?;

        Ok(format!("COPY: loaded ~{row_count} rows from '{file_path}'"))
    } else {
        Err(pgcli_rs::error::PgCliError::Config(
            "\\copy: expected TO or FROM in spec (e.g. \\copy TABLE TO 'file')".to_string(),
        ))
    }
}

/// Find a keyword (case-sensitive on pre-uppercased string) at a word boundary.
fn find_keyword_pos(hay_upper: &str, needle: &str) -> Option<usize> {
    hay_upper.find(needle)
}

/// Extract a file path (possibly quoted with `'` or `"`) from the start of `s`.
/// Returns `(path, remainder)`.
fn extract_path(s: &str) -> pgcli_rs::Result<(String, String)> {
    let s = s.trim_start();
    if s.starts_with('\'') || s.starts_with('"') {
        let q = s.chars().next().unwrap();
        let end = s[1..].find(q).ok_or_else(|| {
            pgcli_rs::error::PgCliError::Config(format!("unclosed quote in \\copy path: {s}"))
        })?;
        let path = s[1..end + 1].to_string();
        let rest = s[end + 2..].trim().to_string();
        Ok((path, rest))
    } else {
        // Unquoted path: take until whitespace
        let end = s.find(char::is_whitespace).unwrap_or(s.len());
        Ok((s[..end].to_string(), s[end..].trim().to_string()))
    }
}

/// Apply a `\pset KEY [VALUE]` option to the mutable format and options state.
///
/// Also updates `dispatcher` for options that mirror interactive toggles (`\x`, `\timing`).
fn apply_pset(
    key: &str,
    value: Option<String>,
    format: &mut OutputFormat,
    opts: &mut pgcli_rs::output::formats::FormatOptions,
    dispatcher: &mut MetaCommandDispatcher,
) {
    use pgcli_rs::output::formats::LineStyle;
    match key {
        "format" => match value {
            Some(ref v) => match v.parse::<OutputFormat>() {
                Ok(f) => {
                    *format = f;
                    dispatcher.format = v.clone();
                    println!("Output format is now '{v}'.");
                }
                Err(()) => eprintln!("\\pset: unknown format '{v}' (use: table csv json jsonl tsv html unaligned markdown latex asciidoc)"),
            },
            None => println!("Output format: {}", format.as_str()),
        },
        "null" => match value {
            Some(v) => {
                println!("Null display is '{v}'.");
                opts.null_display = v;
            }
            None => println!("Null display is '{}'.", opts.null_display),
        },
        "border" => match value.as_deref().and_then(|s| s.parse::<u8>().ok()) {
            Some(n) if n <= 2 => {
                opts.border = n;
                println!("Border style is {n}.");
            }
            _ => eprintln!("\\pset border: expected 0, 1, or 2"),
        },
        "title" => {
            opts.title = value.filter(|s| !s.is_empty());
            match &opts.title {
                Some(t) => println!("Title is '{t}'."),
                None => println!("Title cleared."),
            }
        }
        "footer" => {
            opts.footer = parse_on_off(value.as_deref(), opts.footer);
            println!("Footer is {}.", on_off(opts.footer));
        }
        "columns" | "C" => {
            let n = value.as_deref().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
            opts.max_column_width = n;
            if n == 0 {
                println!("Column width: unlimited.");
            } else {
                println!("Column width: {n}.");
            }
        }
        "tuples_only" | "t" => {
            opts.tuples_only = parse_on_off(value.as_deref(), opts.tuples_only);
            println!("Tuples-only mode is {}.", on_off(opts.tuples_only));
        }
        "expanded" | "x" => {
            opts.expanded = parse_on_off(value.as_deref(), opts.expanded);
            dispatcher.expanded = opts.expanded;
            println!("Expanded display is {}.", on_off(opts.expanded));
        }
        "timing" => {
            opts.timing = parse_on_off(value.as_deref(), opts.timing);
            dispatcher.timing = opts.timing;
            println!("Timing is {}.", on_off(opts.timing));
        }
        "fieldsep" | "F" => {
            let v = value.unwrap_or_else(|| "|".to_string());
            println!("Field separator is '{v}'.");
            opts.field_separator = v;
        }
        "recordsep" | "R" => {
            opts.record_separator = value.unwrap_or_else(|| "\n".to_string());
        }
        "linestyle" => match value.as_deref() {
            Some("ascii") => { opts.line_style = LineStyle::Ascii; println!("Line style is 'ascii'."); }
            Some("old-ascii") => { opts.line_style = LineStyle::OldAscii; println!("Line style is 'old-ascii'."); }
            Some("unicode") | None => { opts.line_style = LineStyle::Unicode; println!("Line style is 'unicode'."); }
            Some(other) => eprintln!("\\pset linestyle: unknown style '{other}' (use: ascii unicode old-ascii)"),
        },
        "numericlocale" => {
            opts.numeric_locale = parse_on_off(value.as_deref(), opts.numeric_locale);
            println!("Numeric locale is {}.", on_off(opts.numeric_locale));
        }
        "pager" => {
            opts.pager_enabled = parse_on_off(value.as_deref(), opts.pager_enabled);
            println!("Pager usage is {}.", on_off(opts.pager_enabled));
        }
        other => eprintln!("\\pset: unknown option '{other}'. Run \\pset without args for help."),
    }
}

/// Parse `"on"`/`"off"` → bool, defaulting to toggling `current` when value is absent or ambiguous.
fn parse_on_off(value: Option<&str>, current: bool) -> bool {
    match value {
        Some("on") | Some("true") | Some("1") | Some("yes") => true,
        Some("off") | Some("false") | Some("0") | Some("no") => false,
        _ => !current,
    }
}

fn on_off(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

/// Prompt the user for a password without echoing input.
fn prompt_password(prompt: &str) -> pgcli_rs::Result<String> {
    rpassword::prompt_password(prompt).map_err(|e| {
        pgcli_rs::error::PgCliError::Connection(format!("password prompt failed: {e}"))
    })
}
