//! Handling of the result of a backslash meta-command in the REPL.

use std::io::Write as _;
use std::time::Duration;

use pgcli_executor::{query::QueryExecutor, runner::ScriptRunner};
use pgcli_meta::{
    commands::MetaResult,
    introspection::{Introspector, ObjectType},
};
use pgcli_output::{formats::format_result, stats::BenchStats};
use pgcli_repl::schema_cache::SchemaCache;

use crate::copy::client_copy;
use crate::crosstab::crosstab_pivot;
use crate::pset::apply_pset;
use crate::session::{Flow, Session};
use crate::settings::effective_expanded;
use crate::shell::run_shell;

impl Session<'_> {
    /// Acts on the result of a dispatched meta-command. Errors are printed;
    /// only `\q` ends the session.
    pub async fn handle_meta(&mut self, result: pgcli_core::Result<MetaResult>) -> Flow {
        match result {
            Ok(MetaResult::Quit) => {
                println!("Bye.");
                return Flow::Quit;
            }
            Ok(MetaResult::Output(s)) => println!("{s}"),
            Ok(MetaResult::ChangeTheme(t)) => {
                self.opts.theme = t.clone();
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
                    .unwrap_or_else(|| self.pool.config().database.clone());
                match self.pool.reconnect_to(dbname, user, host, port).await {
                    Ok(()) => {
                        println!(
                            "You are now connected to database \"{}\" as user \"{}\".",
                            self.pool.config().database,
                            self.pool.config().user
                        );
                        // Refresh the schema cache for the new database in the background.
                        SchemaCache::spawn_refresh(
                            self.schema_cache.clone(),
                            self.pool.shared_client(),
                        );
                    }
                    Err(e) => {
                        eprintln!("ERROR: could not connect to \"{db_display}\": {e}")
                    }
                }
                // Reset last_sql so \watch doesn't rerun the previous db's query.
                self.dispatcher.last_sql.clear();
            }
            Ok(MetaResult::ExecuteFile(path)) => {
                let mut runner = ScriptRunner::new();
                for (k, v) in &self.dispatcher.variables {
                    runner.set_variable(k, v);
                }
                match runner.run_file(self.pool.client(), &path, false).await {
                    Ok(results) => {
                        for result in results {
                            self.opts.expanded = self.dispatcher.expanded;
                            self.opts.timing = self.dispatcher.timing;
                            let out = format_result(&result, &self.format, &self.opts)
                                .unwrap_or_default();
                            self.pager.print(&out).ok();
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
                        return Flow::Continue;
                    }
                };
                let mut runner = ScriptRunner::new();
                for (k, v) in &self.dispatcher.variables {
                    runner.set_variable(k, v);
                }
                for stmt in content.split(';') {
                    let s = stmt.trim();
                    if s.is_empty() || s.starts_with("--") {
                        continue;
                    }
                    println!("-- {s}");
                    match QueryExecutor::execute_batch(self.pool.client(), &format!("{s};")).await {
                        Ok(results) => {
                            for result in results {
                                self.opts.expanded = self.dispatcher.expanded;
                                self.opts.timing = self.dispatcher.timing;
                                let out = format_result(&result, &self.format, &self.opts)
                                    .unwrap_or_default();
                                self.pager.print(&out).ok();
                            }
                        }
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
            }
            Ok(MetaResult::WriteResult(path)) => {
                if self.dispatcher.last_sql.is_empty() {
                    eprintln!("\\write: no previous query to write.");
                } else {
                    match QueryExecutor::execute_batch(
                        self.pool.client(),
                        &self.dispatcher.last_sql.clone(),
                    )
                    .await
                    {
                        Ok(results) => {
                            let mut combined = String::new();
                            for result in &results {
                                self.opts.expanded = self.dispatcher.expanded;
                                self.opts.timing = false;
                                combined.push_str(
                                    &format_result(result, &self.format, &self.opts)
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
                let sql = self.dispatcher.last_sql.clone();
                println!("Watching every {interval_secs}s - press Ctrl-C to stop.\n");
                loop {
                    match QueryExecutor::execute_batch(self.pool.client(), &sql).await {
                        Ok(results) => {
                            // Clear screen for watch mode.
                            print!("\x1b[2J\x1b[H");
                            let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                            println!("-- {sql}  [{now}] (every {interval_secs}s)");
                            for result in &results {
                                self.opts.expanded = effective_expanded(&self.dispatcher, result);
                                self.opts.timing = self.dispatcher.timing;
                                let out = format_result(result, &self.format, &self.opts)
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
                let introspector = Introspector::new(self.pool.client());
                match introspector.describe_table(&schema, &name).await {
                    Ok(desc) => {
                        self.opts.expanded = self.dispatcher.expanded;
                        self.opts.timing = false;
                        let out =
                            format_result(&desc.raw, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
                        // Append constraint section (PK, UNIQUE, CHECK).
                        match introspector
                            .describe_table_constraints(&schema, &name)
                            .await
                        {
                            Ok(cs) if !cs.is_empty() => {
                                self.pager.print(&cs).ok();
                            }
                            _ => {}
                        }
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::DdlTable { schema, name }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector
                    .generate_ddl(ObjectType::Table, &schema, &name)
                    .await
                {
                    Ok(ddl) => {
                        self.pager.print(&ddl).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ShowFunctionSource { schema, name }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_function_source(&schema, &name).await {
                    Ok(src) => {
                        self.pager.print(&src).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ShowViewDefinition { schema, name }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_view_definition(&schema, &name).await {
                    Ok(def) => {
                        self.pager.print(&def).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::IntrospectTableExtended { schema, name }) => {
                let introspector = Introspector::new(self.pool.client());
                // Base column listing.
                match introspector.describe_table(&schema, &name).await {
                    Ok(desc) => {
                        self.opts.expanded = self.dispatcher.expanded;
                        self.opts.timing = false;
                        let out =
                            format_result(&desc.raw, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
                // Extended sections: constraints, FKs, triggers.
                match introspector.describe_table_extended(&schema, &name).await {
                    Ok(ext) => {
                        self.pager.print(&ext).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::SetPrintOption { key, value }) => match key.as_str() {
                "prompt1" => {
                    self.editor.prompt1 = value.filter(|v| !v.is_empty());
                    println!("PROMPT1 updated.");
                }
                "prompt2" => {
                    self.editor.prompt2 = value.filter(|v| !v.is_empty());
                    println!("PROMPT2 updated.");
                }
                _ => {
                    apply_pset(
                        &key,
                        value,
                        &mut self.format,
                        &mut self.opts,
                        &mut self.dispatcher,
                    );
                    self.pager.disabled = !self.opts.pager_enabled;
                }
            },
            Ok(MetaResult::GSet { prefix }) => {
                let sql = self.dispatcher.last_sql.clone();
                match QueryExecutor::execute(self.pool.client(), &sql).await {
                    Ok(result) => {
                        if let Some(row) = result.rows.first() {
                            let count = result.columns.len();
                            for (col, val) in result.columns.iter().zip(row.values.iter()) {
                                let key = format!("{}{}", prefix, col.name);
                                let value = val.to_string();
                                self.dispatcher.variables.insert(key, value);
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
                let sql = self.dispatcher.last_sql.clone();
                if sql.is_empty() {
                    eprintln!("\\crosstabview: no previous query.");
                } else {
                    match QueryExecutor::execute(self.pool.client(), &sql).await {
                        Ok(result) => match crosstab_pivot(&result, &col_h, &col_v, &col_d) {
                            Ok(out) => {
                                self.pager.print(&out).ok();
                            }
                            Err(e) => eprintln!("\\crosstabview: {e}"),
                        },
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
            }
            Ok(MetaResult::GDesc) => {
                let sql = format!(
                    "SELECT * FROM ({}) __pgcli_q LIMIT 0",
                    self.dispatcher.last_sql
                );
                match QueryExecutor::execute(self.pool.client(), &sql).await {
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
                let sql = self.dispatcher.last_sql.clone();
                println!(
                    "Benchmarking {count} run(s): {}{}",
                    sql.chars().take(60).collect::<String>(),
                    if sql.chars().count() > 60 { "..." } else { "" }
                );
                let mut bench = BenchStats::new(count);
                let mut total_rows = 0usize;
                for _ in 0..count {
                    let t0 = std::time::Instant::now();
                    match QueryExecutor::execute(self.pool.client(), &sql).await {
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
                let last = self.dispatcher.last_sql.clone();
                let new_sql = tokio::task::spawn_blocking(move || {
                    let tmp = std::env::temp_dir().join("pgcli_edit.sql");
                    std::fs::write(&tmp, &last).ok();
                    let editor_cmd = std::env::var("VISUAL")
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
                        .args(["/C", "start", "/wait", "", &editor_cmd])
                        .arg(&tmp)
                        .status();
                    #[cfg(not(windows))]
                    let _ = std::process::Command::new(&editor_cmd).arg(&tmp).status();
                    std::fs::read_to_string(&tmp).unwrap_or_default()
                })
                .await
                .unwrap_or_default();

                let new_sql = new_sql.trim().to_string();
                if new_sql.is_empty() {
                    println!("(empty buffer - nothing executed)");
                } else {
                    self.dispatcher.last_sql = new_sql.clone();
                    match QueryExecutor::execute_batch(self.pool.client(), &new_sql).await {
                        Ok(results) => {
                            for result in results {
                                self.opts.expanded = self.dispatcher.expanded;
                                self.opts.timing = self.dispatcher.timing;
                                let out = format_result(&result, &self.format, &self.opts)
                                    .unwrap_or_default();
                                self.pager.print(&out).ok();
                            }
                        }
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
            }
            Ok(MetaResult::Query(sql)) => {
                match QueryExecutor::execute(self.pool.client(), &sql).await {
                    Ok(result) => {
                        self.opts.expanded = effective_expanded(&self.dispatcher, &result);
                        self.opts.timing = self.dispatcher.timing;
                        if self.args.echo_hidden {
                            eprintln!(">> {sql}");
                        }
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
                        } else {
                            format_result(&result, &self.format, &self.opts).unwrap_or_default()
                        };
                        self.pager.print(&out).ok();
                    }
                    Err(e) => {
                        eprintln!("ERROR: {e}");
                        if self.args.echo_errors {
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
                    match QueryExecutor::execute(self.pool.client(), &sql).await {
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
                match QueryExecutor::execute(self.pool.client(), &sql).await {
                    Ok(res) => {
                        use pgcli_core::protocol::messages::CellValue;
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
                let sql = self.dispatcher.last_sql.clone();
                if sql.is_empty() {
                    eprintln!("\\g: no previous query.");
                } else {
                    match QueryExecutor::execute_batch(self.pool.client(), &sql).await {
                        Ok(results) => {
                            for result in results {
                                self.opts.expanded = effective_expanded(&self.dispatcher, &result);
                                self.opts.timing = self.dispatcher.timing;
                                let out = if result.columns.is_empty() {
                                    result.command_tag.clone()
                                } else {
                                    format_result(&result, &self.format, &self.opts)
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
                                    self.pager.print(&out).ok();
                                }
                            }
                        }
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
            }
            Ok(MetaResult::RepeatExpanded) => {
                let sql = self.dispatcher.last_sql.clone();
                if sql.is_empty() {
                    eprintln!("\\gx: no previous query.");
                } else {
                    match QueryExecutor::execute_batch(self.pool.client(), &sql).await {
                        Ok(results) => {
                            for result in results {
                                self.opts.expanded = true;
                                self.opts.timing = self.dispatcher.timing;
                                let out = format_result(&result, &self.format, &self.opts)
                                    .unwrap_or_default();
                                self.pager.print(&out).ok();
                            }
                        }
                        Err(e) => eprintln!("ERROR: {e}"),
                    }
                }
            }
            Ok(MetaResult::GExec) => {
                let sql = self.dispatcher.last_sql.clone();
                match QueryExecutor::execute(self.pool.client(), &sql).await {
                    Ok(result) => {
                        let mut executed = 0usize;
                        for row in &result.rows {
                            for cell in &row.values {
                                let cell_sql = cell.to_string();
                                if cell_sql.is_empty() {
                                    continue;
                                }
                                match QueryExecutor::execute(self.pool.client(), &cell_sql).await {
                                    Ok(_) => {
                                        executed += 1;
                                        println!("OK: {}", &cell_sql[..cell_sql.len().min(80)]);
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
                match client_copy(self.pool.client(), &spec).await {
                    Ok(msg) => println!("{msg}"),
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ShowDeps { name }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_deps(&name).await {
                    Ok(out) => self.pager.print(&out).ok(),
                    Err(e) => {
                        eprintln!("ERROR: {e}");
                        None
                    }
                };
            }
            Ok(MetaResult::WatchDiff { interval_secs }) => {
                let sql = self.dispatcher.last_sql.clone();
                println!("Watching (diff) every {interval_secs}s — Ctrl-C to stop.\n");
                let mut prev_lines: Vec<String> = Vec::new();
                loop {
                    match QueryExecutor::execute_batch(self.pool.client(), &sql).await {
                        Ok(results) => {
                            let mut cur_lines: Vec<String> = Vec::new();
                            for result in &results {
                                let out = format_result(result, &self.format, &self.opts)
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
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_indexes(&name).await {
                    Ok(result) => {
                        self.opts.expanded = false;
                        self.opts.timing = false;
                        let out =
                            format_result(&result, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ShowBloat) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_bloat().await {
                    Ok(result) => {
                        self.opts.expanded = false;
                        self.opts.timing = false;
                        let out =
                            format_result(&result, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ShowColumnStats { schema, name }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_column_stats(&schema, &name).await {
                    Ok(result) => {
                        if result.rows.is_empty() {
                            eprintln!("No statistics for {schema}.{name} (run ANALYZE first).");
                        } else {
                            self.opts.expanded = false;
                            self.opts.timing = false;
                            let out = format_result(&result, &self.format, &self.opts)
                                .unwrap_or_default();
                            self.pager.print(&out).ok();
                        }
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ShowPartitions { schema, name }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.show_partitions(&schema, &name).await {
                    Ok(result) => {
                        if result.rows.is_empty() {
                            println!(
                                "{schema}.{name} is not a partitioned table or has no partitions."
                            );
                        } else {
                            self.opts.expanded = false;
                            self.opts.timing = false;
                            let out = format_result(&result, &self.format, &self.opts)
                                .unwrap_or_default();
                            self.pager.print(&out).ok();
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
                self.dispatcher.on_error_mode = mode.clone();
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
                self.dispatcher.variables.insert(var, val);
            }
            Ok(MetaResult::ListRoles { pattern }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.list_roles(&pattern).await {
                    Ok(result) => {
                        self.opts.expanded = false;
                        self.opts.timing = false;
                        let out =
                            format_result(&result, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::ListSequences { pattern }) => {
                let introspector = Introspector::new(self.pool.client());
                match introspector.list_sequences(&pattern).await {
                    Ok(result) => {
                        self.opts.expanded = false;
                        self.opts.timing = false;
                        let out =
                            format_result(&result, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
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
                match QueryExecutor::execute(self.pool.client(), &sql).await {
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
                let introspector = Introspector::new(self.pool.client());
                match introspector.list_languages(&pattern).await {
                    Ok(result) => {
                        self.opts.expanded = false;
                        self.opts.timing = false;
                        let out =
                            format_result(&result, &self.format, &self.opts).unwrap_or_default();
                        self.pager.print(&out).ok();
                    }
                    Err(e) => eprintln!("ERROR: {e}"),
                }
            }
            Ok(MetaResult::Ok) => {}
            Err(e) => eprintln!("{e}"),
        }
        Flow::Continue
    }
}
