/// Backslash meta-command parsing and dispatch.
///
/// Implements all standard `psql` meta-commands plus pgcli-rs extensions.
use std::path::PathBuf;
use std::str::SplitWhitespace;

use crate::error::{PgCliError, Result};
use crate::meta::bookmarks;

/// A parsed meta-command with its name and raw argument string.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaCommand {
    /// The command name without the leading backslash (e.g. `"d"`, `"dt"`, `"q"`).
    pub name: String,
    /// The remainder of the line after the command name, trimmed.
    pub args: String,
}

impl MetaCommand {
    /// Parse a meta-command line (with or without the leading `\`).
    ///
    /// Returns `None` if `line` does not begin with `\`.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        let line = line.strip_prefix('\\')?;
        let mut parts = line.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").to_string();
        let args = parts.next().unwrap_or("").trim().to_string();
        if name.is_empty() {
            return None;
        }
        Some(Self { name, args })
    }

    /// Return an iterator over whitespace-separated arguments.
    pub fn arg_tokens(&self) -> SplitWhitespace<'_> {
        self.args.split_whitespace()
    }

    /// Return the first argument token, if any.
    pub fn first_arg(&self) -> Option<&str> {
        self.args.split_whitespace().next()
    }
}

/// The result of dispatching a meta-command.
#[derive(Debug, Clone, PartialEq)]
pub enum MetaResult {
    /// The command produced output to display.
    Output(String),
    /// The command requires a SQL query to be executed and displayed.
    Query(String),
    /// The user requested a graceful exit.
    Quit,
    /// The command succeeded silently (no output).
    Ok,
    /// Reconnect to a (possibly different) database/user/host.
    Reconnect {
        dbname: Option<String>,
        user: Option<String>,
        host: Option<String>,
        port: Option<u16>,
    },
    /// Execute a SQL script file and display results.
    ExecuteFile(PathBuf),
    /// Repeat the last SQL every `interval_secs` seconds until Ctrl-C.
    Watch { interval_secs: u64 },
    /// Describe a specific table's columns (async - handled by caller).
    IntrospectTable { schema: String, name: String },
    /// Generate and display DDL for a named table (async - handled by caller).
    DdlTable { schema: String, name: String },
    /// Apply a new color theme.
    ChangeTheme(String),
    /// Store the first row of the last query as dispatcher variables.
    GSet { prefix: String },
    /// Benchmark the last query `count` times and print statistics.
    Bench { count: u32 },
    /// Open the query buffer in `$EDITOR`, then execute on save.
    EditAndExecute,
    /// Show a column-type description of what the last query returns without running it.
    GDesc,
    /// Update a print/formatting option at runtime (`\pset` or `\format`).
    SetPrintOption {
        /// Option name (e.g. `"null"`, `"border"`, `"format"`, `"title"`).
        key: String,
        /// New value, or `None` to show the current value.
        value: Option<String>,
    },
    /// Display the source of a stored function or procedure (`\sf`).
    ShowFunctionSource {
        /// Schema name (defaults to `public`).
        schema: String,
        /// Function name.
        name: String,
    },
    /// Display the definition of a view (`\sv`).
    ShowViewDefinition {
        /// Schema name (defaults to `public`).
        schema: String,
        /// View name.
        name: String,
    },
    /// Extended table description: base columns plus constraints, FKs, and triggers (`\d+`).
    IntrospectTableExtended {
        /// Schema name.
        schema: String,
        /// Table name.
        name: String,
    },
    /// Pause execution for `secs` seconds (`\sleep N`).
    Sleep(f64),
    /// Write last query result to a file in the current output format.
    WriteResult(String),
    /// Run a SQL file and print each statement before executing.
    ExecuteFileVerbose(std::path::PathBuf),
    /// Import a local file as a PostgreSQL large object.
    LoImport(String),
    /// Export a PostgreSQL large object to a local file.
    LoExport { oid: u32, path: String },
    /// Pivot the last query result as a cross-tab view (`\crosstabview`).
    CrossTabView {
        /// Column name to use as horizontal header (defaults to col 0).
        col_h: String,
        /// Column name to use as vertical header (defaults to col 1).
        col_v: String,
        /// Column name to use as cell data (defaults to col 2).
        col_d: String,
    },
    /// Re-execute the last SQL, optionally writing output to a file (`\g [FILE]`).
    Repeat {
        /// If `Some`, redirect output to this file path.
        to_file: Option<String>,
    },
    /// Re-execute the last SQL in expanded display mode (`\gx`).
    RepeatExpanded,
    /// Print output without trailing newline (`\echo -n TEXT`).
    OutputNoNl(String),
    /// Execute each cell of the last query result as a SQL statement (`\gexec`).
    GExec,
    /// Client-side COPY to/from a local file (`\copy`).
    ///
    /// The argument string is everything after `\copy` (e.g. `"TABLE TO 'file.csv' WITH (FORMAT CSV)"`).
    ClientCopy(String),
    /// Show dependencies of a database object (`\deps`).
    ShowDeps {
        /// Object name to look up (schema.name or just name).
        name: String,
    },
    /// Repeat last query in diff mode — shows lines added/removed vs previous run (`\watch diff`).
    WatchDiff { interval_secs: u64 },
    /// Show detailed index information for a table or all user tables (`\indexes`).
    ShowIndexes {
        /// Optional table name (schema.name or just name). Empty = all user tables.
        name: String,
    },
    /// Estimate table and index bloat from system catalogs (`\bloat`).
    ShowBloat,
    /// Show column statistics from `pg_stats` for a table (`\stats SCHEMA.TABLE`).
    ShowColumnStats {
        /// Schema name (defaults to "public").
        schema: String,
        /// Table name.
        name: String,
    },
    /// Show partition list for a partitioned table (`\partitions SCHEMA.TABLE`).
    ShowPartitions {
        /// Schema name.
        schema: String,
        /// Table name.
        name: String,
    },
    /// Execute a shell command and print its output (`\!`).
    ShellExec(String),
    /// Print a message to stderr (`\warn`).
    Warn(String),
    /// Set the `\on_error` mode for script execution.
    SetOnError(String),
    /// Interactively prompt the user for a variable value (`\prompt`).
    Prompt {
        /// Prompt text shown to the user.
        text: String,
        /// Variable name to store the response in.
        var: String,
    },
    /// Show roles/users (`\du` / `\dg`).
    ListRoles {
        /// Optional ILIKE pattern.
        pattern: String,
    },
    /// Show sequences (`\ds` extended).
    ListSequences {
        /// Optional ILIKE pattern.
        pattern: String,
    },
    /// Cancel or terminate a backend process (`\kill`).
    KillBackend {
        /// Process ID.
        pid: i32,
        /// If true use `pg_terminate_backend`, else `pg_cancel_backend`.
        force: bool,
    },
    /// Show procedural languages (`\dL`).
    ListLanguages {
        /// Optional ILIKE pattern.
        pattern: String,
    },
}

/// Dispatches parsed `MetaCommand`s to their implementations.
///
/// State held here includes display options, variable bindings, the
/// last-executed SQL for `\watch`, and the named bookmark store.
pub struct MetaCommandDispatcher {
    /// Current print option: expanded display mode (`on`, `off`, or `auto`).
    pub expanded: bool,
    /// When true, use "auto" expanded mode: expand rows wider than terminal.
    pub expanded_auto: bool,
    /// Current print option: show timing.
    pub timing: bool,
    /// Current output file path (None = stdout).
    pub output_file: Option<PathBuf>,
    /// psql variable store (`\set` / `-v`).
    pub variables: std::collections::HashMap<String, String>,
    /// Current output format name.
    pub format: String,
    /// Active color theme: `dark`, `light`, or `none`.
    pub theme: String,
    /// Most recent SQL statement (used by `\watch`, `\bench`, `\gset`).
    pub last_sql: String,
    /// Named query bookmarks (name -> SQL), persisted to `~/.pgcli_bookmarks.toml`.
    pub bookmarks: std::collections::HashMap<String, String>,
    /// Error handling mode for scripts: `"stop"` (default), `"continue"`, or `"rollback"`.
    pub on_error_mode: String,
}

impl Default for MetaCommandDispatcher {
    fn default() -> Self {
        Self {
            expanded: false,
            expanded_auto: false,
            timing: true,
            output_file: None,
            variables: std::collections::HashMap::new(),
            format: "table".to_string(),
            theme: "dark".to_string(),
            last_sql: String::new(),
            bookmarks: bookmarks::load(),
            on_error_mode: "stop".to_string(),
        }
    }
}

impl MetaCommandDispatcher {
    /// Create a new dispatcher with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Substitute `:varname` tokens in `sql` with values from `self.variables`.
    ///
    /// Only whole-word `:name` patterns are replaced; `:name` inside literals or
    /// identifiers is also substituted (same behaviour as psql).
    pub fn substitute_vars(&self, sql: &str) -> String {
        if self.variables.is_empty() || !sql.contains(':') {
            return sql.to_string();
        }
        let mut result = sql.to_string();
        // Sort by descending key length so longer names shadow prefixes.
        let mut pairs: Vec<(&String, &String)> = self.variables.iter().collect();
        pairs.sort_by_key(|b| std::cmp::Reverse(b.0.len()));
        for (k, v) in pairs {
            // :'name' → 'value' (psql single-quoted string literal substitution)
            let escaped_sq = v.replace('\'', "''");
            result = result.replace(&format!(":'{}'", k), &format!("'{}'", escaped_sq));
            // :"name" → "value" (psql double-quoted identifier substitution)
            let escaped_dq = v.replace('"', "\"\"");
            result = result.replace(&format!(":\"{}\"", k), &format!("\"{}\"", escaped_dq));
            // :name → value (bare substitution, done last so the quoted forms take priority)
            result = result.replace(&format!(":{k}"), v);
        }
        result
    }

    /// Dispatch `cmd` and return its result.
    ///
    /// Commands that require database interaction return `MetaResult::Query(sql)`.
    /// Async operations (introspect, ddl) return their own variants for the caller
    /// to handle with a live client.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Config` for invalid command arguments.
    pub fn dispatch(&mut self, cmd: &MetaCommand) -> Result<MetaResult> {
        match cmd.name.as_str() {
            // -- Exit --------------------------------------------------------
            "q" | "quit" | "q!" | "exit" => Ok(MetaResult::Quit),

            // -- Connection --------------------------------------------------
            "c" | "connect" => {
                let mut tokens = cmd.arg_tokens();
                let dbname = tokens.next().map(|s| s.to_string()).filter(|s| s != "-");
                let user   = tokens.next().map(|s| s.to_string()).filter(|s| s != "-");
                let host   = tokens.next().map(|s| s.to_string()).filter(|s| s != "-");
                let port   = tokens.next().and_then(|s| s.parse::<u16>().ok());
                Ok(MetaResult::Reconnect { dbname, user, host, port })
            }

            // Reconnect to the same server (e.g. after a connection drop).
            "reconnect" => Ok(MetaResult::Reconnect {
                dbname: None,
                user: None,
                host: None,
                port: None,
            }),

            "conninfo" => Ok(MetaResult::Query(conninfo_query())),

            "password" => {
                let who = cmd.first_arg().unwrap_or("");
                Ok(MetaResult::Output(format!(
                    "Use ALTER ROLE {who} WITH PASSWORD '...' to change a password."
                )))
            }

            // -- Databases / schemas -----------------------------------------
            "l" | "list" => Ok(MetaResult::Query(list_databases_query())),
            "l+" => Ok(MetaResult::Query(list_databases_query_extended())),

            // -- Object descriptions -----------------------------------------
            "d" | "describe" => {
                match cmd.first_arg() {
                    Some(name) if !name.contains('*') && !name.contains('%') && !name.contains('?') => {
                        // Exact name - show column details.
                        let (schema, tbl) = split_schema_table(name);
                        Ok(MetaResult::IntrospectTable { schema, name: tbl })
                    }
                    pattern => Ok(MetaResult::Query(describe_query(pattern))),
                }
            }
            "d+" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\d+ requires a table name".to_string())
                })?;
                let (schema, tbl) = split_schema_table(name);
                Ok(MetaResult::IntrospectTableExtended { schema, name: tbl })
            }
            "dt" => Ok(MetaResult::Query(list_tables_query(cmd.first_arg()))),
            "dv" => Ok(MetaResult::Query(list_views_query(cmd.first_arg()))),
            "dm" => Ok(MetaResult::Query(list_matviews_query(cmd.first_arg()))),
            "di" => Ok(MetaResult::Query(list_indexes_query(cmd.first_arg()))),
            "ds" => Ok(MetaResult::Query(list_sequences_query(cmd.first_arg()))),
            "df" => Ok(MetaResult::Query(list_functions_query(cmd.first_arg()))),
            "df+" => Ok(MetaResult::Query(list_functions_query_extended(cmd.first_arg()))),
            "dn" => Ok(MetaResult::Query(list_schemas_query(cmd.first_arg()))),
            "dn+" => Ok(MetaResult::Query(list_schemas_extended_query(cmd.first_arg()))),
            "dC" => Ok(MetaResult::Query(list_casts_query(cmd.first_arg()))),
            "dy" => Ok(MetaResult::Query(list_event_triggers_query(cmd.first_arg()))),
            "dY" => Ok(MetaResult::Query(list_event_triggers_query(cmd.first_arg()))),
            "dT+" => Ok(MetaResult::Query(list_types_query_extended(cmd.first_arg()))),
            // -- Source viewers -----------------------------------------------
            "sf" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\sf requires a function name".to_string())
                })?;
                let (schema, func) = split_schema_table(name);
                Ok(MetaResult::ShowFunctionSource { schema, name: func })
            }
            "sv" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\sv requires a view name".to_string())
                })?;
                let (schema, view) = split_schema_table(name);
                Ok(MetaResult::ShowViewDefinition { schema, name: view })
            }
            // -- Client encoding ----------------------------------------------
            "encoding" => {
                let sql = if let Some(enc) = cmd.first_arg() {
                    let enc_esc = enc.replace('\'', "''");
                    format!("SET client_encoding TO '{enc_esc}'")
                } else {
                    "SHOW client_encoding".to_string()
                };
                Ok(MetaResult::Query(sql))
            }
            "du" | "dg" => {
                let pattern = cmd.first_arg().unwrap_or("").to_string();
                Ok(MetaResult::ListRoles { pattern })
            }
            "dp" | "z" => Ok(MetaResult::Query(list_privileges_query(cmd.first_arg()))),
            // -- GUC parameter listing ----------------------------------------
            "dconfig" | "dc" => {
                let filter = cmd.first_arg()
                    .map(|p| {
                        let p_esc = p.replace('\'', "''").replace('%', "%%");
                        format!("WHERE name ILIKE '%{p_esc}%'")
                    })
                    .unwrap_or_default();
                Ok(MetaResult::Query(format!(
                    "SELECT name AS \"Parameter\", setting AS \"Value\", \
                     unit AS \"Unit\", category AS \"Category\", \
                     short_desc AS \"Description\" \
                     FROM pg_catalog.pg_settings \
                     {filter} \
                     ORDER BY category, name;"
                )))
            }
            // -- Execute each result cell as SQL ------------------------------
            // \g [FILE]: execute the last SQL (optionally writing output to FILE).
            "g" => {
                if self.last_sql.is_empty() {
                    Ok(MetaResult::Output("\\g: no previous query in buffer.".to_string()))
                } else {
                    let file = cmd.first_arg().map(|s| s.to_string());
                    Ok(MetaResult::Repeat { to_file: file })
                }
            }
            // \gx: execute last SQL in expanded mode.
            "gx" => {
                if self.last_sql.is_empty() {
                    Ok(MetaResult::Output("\\gx: no previous query in buffer.".to_string()))
                } else {
                    Ok(MetaResult::RepeatExpanded)
                }
            }
            "gexec" => {
                if self.last_sql.is_empty() {
                    Err(PgCliError::Config(
                        "\\gexec: no previous query to use".to_string(),
                    ))
                } else {
                    Ok(MetaResult::GExec)
                }
            }
            // -- Client-side COPY (local file) --------------------------------
            "copy" => {
                if cmd.args.is_empty() {
                    return Err(PgCliError::Config(
                        "\\copy requires arguments: TABLE TO/FROM 'file' [WITH ...]".to_string(),
                    ));
                }
                Ok(MetaResult::ClientCopy(cmd.args.clone()))
            }
            // -- Large-object meta-commands -----------------------------------
            "lo_list" | "lo_list+" => Ok(MetaResult::Query(
                "SELECT loid AS \"OID\", pg_catalog.obj_description(loid,'pg_largeobject') AS \"Description\" \
                 FROM pg_catalog.pg_largeobject_metadata ORDER BY loid;".to_string()
            )),
            "lo_import" => {
                if cmd.args.is_empty() {
                    Err(PgCliError::Config("\\lo_import requires a file path".to_string()))
                } else {
                    Ok(MetaResult::LoImport(cmd.args.trim().to_string()))
                }
            }
            "lo_export" => {
                let parts: Vec<&str> = cmd.args.splitn(2, ' ').collect();
                if parts.len() < 2 {
                    Err(PgCliError::Config("Usage: \\lo_export OID PATH".to_string()))
                } else {
                    let oid: u32 = parts[0].trim().parse().map_err(|_| {
                        PgCliError::Config("\\lo_export: OID must be a number".to_string())
                    })?;
                    Ok(MetaResult::LoExport { oid, path: parts[1].trim().to_string() })
                }
            }
            "lo_unlink" => {
                let oid: u32 = cmd.args.trim().parse().map_err(|_| {
                    PgCliError::Config("\\lo_unlink: OID must be a number".to_string())
                })?;
                Ok(MetaResult::Query(format!("SELECT lo_unlink({oid});")))
            }
            "dT" => Ok(MetaResult::Query(list_types_query(cmd.first_arg()))),
            "dx" => Ok(MetaResult::Query(list_extensions_query(cmd.first_arg()))),
            "dA" => Ok(MetaResult::Query(list_access_methods_query(cmd.first_arg()))),
            "dRs" | "drs" => Ok(MetaResult::Query(list_role_grants_query(cmd.first_arg()))),
            "dRg" | "drg" => Ok(MetaResult::Query(list_role_memberships_query(cmd.first_arg()))),
            "dD" | "dd" => Ok(MetaResult::Query(list_domains_query(cmd.first_arg()))),
            "dO" | "do" => Ok(MetaResult::Query(list_collations_query(cmd.first_arg()))),
            "dP" | "dp_partitioned" => Ok(MetaResult::Query(list_partitioned_tables_query(cmd.first_arg()))),
            "dF" => Ok(MetaResult::Query(list_ts_configs_query(cmd.first_arg()))),
            "dFp" => Ok(MetaResult::Query(list_ts_parsers_query(cmd.first_arg()))),
            "dFd" => Ok(MetaResult::Query(list_ts_dicts_query(cmd.first_arg()))),
            "dFt" => Ok(MetaResult::Query(list_ts_templates_query(cmd.first_arg()))),

            // -- Query buffer ------------------------------------------------
            "p" => {
                if self.last_sql.is_empty() {
                    Ok(MetaResult::Output("Query buffer is empty.".to_string()))
                } else {
                    Ok(MetaResult::Output(self.last_sql.clone()))
                }
            }
            "r" => {
                self.last_sql.clear();
                Ok(MetaResult::Output("Query buffer reset.".to_string()))
            }

            // -- Output / formatting -----------------------------------------
            "a" => Ok(MetaResult::Output(
                "Toggle aligned/unaligned: use --format=unaligned or \\format unaligned".to_string()
            )),
            "x" => {
                match cmd.first_arg() {
                    Some("auto") => {
                        self.expanded = false;
                        self.expanded_auto = true;
                        Ok(MetaResult::Output("Expanded display is auto.".to_string()))
                    }
                    Some("on") => {
                        self.expanded = true;
                        self.expanded_auto = false;
                        Ok(MetaResult::Output("Expanded display is on.".to_string()))
                    }
                    Some("off") => {
                        self.expanded = false;
                        self.expanded_auto = false;
                        Ok(MetaResult::Output("Expanded display is off.".to_string()))
                    }
                    _ => {
                        // Toggle: off → on → auto → off
                        if self.expanded_auto {
                            self.expanded = false;
                            self.expanded_auto = false;
                            Ok(MetaResult::Output("Expanded display is off.".to_string()))
                        } else if self.expanded {
                            self.expanded = false;
                            self.expanded_auto = true;
                            Ok(MetaResult::Output("Expanded display is auto.".to_string()))
                        } else {
                            self.expanded = true;
                            Ok(MetaResult::Output("Expanded display is on.".to_string()))
                        }
                    }
                }
            }
            "timing" => {
                let state = match cmd.first_arg() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => !self.timing,
                };
                self.timing = state;
                Ok(MetaResult::Output(format!(
                    "Timing is {}.",
                    if state { "on" } else { "off" }
                )))
            }
            "t" => {
                Ok(MetaResult::Output(
                    "Tuples-only mode: use --tuples-only / -t flag to enable at startup.".to_string()
                ))
            }
            "format" => {
                Ok(MetaResult::SetPrintOption {
                    key: "format".to_string(),
                    value: cmd.first_arg().map(str::to_string),
                })
            }
            "theme" => {
                if let Some(t) = cmd.first_arg() {
                    match t {
                        "dark" | "light" | "none" => {
                            self.theme = t.to_string();
                            Ok(MetaResult::ChangeTheme(t.to_string()))
                        }
                        other => Err(PgCliError::Config(format!(
                            "unknown theme '{other}' - use dark, light, or none"
                        ))),
                    }
                } else {
                    Ok(MetaResult::Output(format!("Current theme: {}", self.theme)))
                }
            }
            "o" => {
                if let Some(f) = cmd.first_arg() {
                    self.output_file = Some(PathBuf::from(f));
                    Ok(MetaResult::Output(format!("Output directed to '{f}'.")))
                } else {
                    self.output_file = None;
                    Ok(MetaResult::Output("Output directed to stdout.".to_string()))
                }
            }
            "pset" => {
                let mut tokens = cmd.arg_tokens();
                let key = tokens.next().unwrap_or("").to_string();
                if key.is_empty() {
                    return Ok(MetaResult::Output(pset_help()));
                }
                // Remaining tokens form the value (allows spaces in title etc.)
                let rest = cmd.args
                    .trim_start_matches(&key)
                    .trim()
                    .to_string();
                let value = if rest.is_empty() { None } else { Some(rest) };
                Ok(MetaResult::SetPrintOption { key, value })
            }

            // -- Variables ---------------------------------------------------
            "set" => {
                let mut tokens = cmd.arg_tokens();
                if let Some(var) = tokens.next() {
                    // Collect remaining tokens as value (allows spaces in value with quotes).
                    let val: String = std::iter::once(tokens.next().unwrap_or(""))
                        .chain(tokens)
                        .collect::<Vec<_>>()
                        .join(" ");
                    // PROMPT1/PROMPT2 are special: routed to editor via SetPrintOption.
                    if var == "PROMPT1" {
                        return Ok(MetaResult::SetPrintOption {
                            key: "prompt1".to_string(),
                            value: Some(val),
                        });
                    }
                    if var == "PROMPT2" {
                        return Ok(MetaResult::SetPrintOption {
                            key: "prompt2".to_string(),
                            value: Some(val),
                        });
                    }
                    self.variables.insert(var.to_string(), val);
                    Ok(MetaResult::Ok)
                } else {
                    if self.variables.is_empty() {
                        return Ok(MetaResult::Output("No variables set.".to_string()));
                    }
                    let mut out = String::new();
                    let mut keys: Vec<&String> = self.variables.keys().collect();
                    keys.sort();
                    for k in keys {
                        out.push_str(&format!("{k} = '{}'\n", self.variables[k]));
                    }
                    Ok(MetaResult::Output(out.trim_end().to_string()))
                }
            }
            "unset" => {
                if let Some(var) = cmd.first_arg() {
                    self.variables.remove(var);
                }
                Ok(MetaResult::Ok)
            }

            // -- Text output -------------------------------------------------
            "echo" | "print" => {
                let text = if cmd.args.starts_with("-n ") {
                    let msg = self.substitute_vars(cmd.args.trim_start_matches("-n "));
                    return Ok(MetaResult::OutputNoNl(msg));
                } else {
                    self.substitute_vars(&cmd.args)
                };
                Ok(MetaResult::Output(text))
            }
            "qecho" => Ok(MetaResult::Output(self.substitute_vars(&cmd.args))),

            // -- Script execution --------------------------------------------
            "i" | "include" => {
                let path = cmd.args.trim().trim_matches('"').trim_matches('\'');
                if path.is_empty() {
                    Err(PgCliError::Config("\\i requires a file path".to_string()))
                } else {
                    Ok(MetaResult::ExecuteFile(PathBuf::from(path)))
                }
            }
            "i+" | "include+" => {
                let path = cmd.args.trim().trim_matches('"').trim_matches('\'');
                if path.is_empty() {
                    Err(PgCliError::Config("\\i+ requires a file path".to_string()))
                } else {
                    Ok(MetaResult::ExecuteFileVerbose(PathBuf::from(path)))
                }
            }
            "ir" | "include_relative" => {
                let path = cmd.args.trim().trim_matches('"').trim_matches('\'');
                if path.is_empty() {
                    Err(PgCliError::Config("\\ir requires a file path".to_string()))
                } else {
                    Ok(MetaResult::ExecuteFile(PathBuf::from(path)))
                }
            }

            // -- Shell -------------------------------------------------------
            "!" => {
                if cmd.args.is_empty() {
                    return Ok(MetaResult::Output(
                        "Usage: \\! COMMAND".to_string()
                    ));
                }
                let output = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
                    .args(if cfg!(windows) {
                        vec!["/C", &cmd.args]
                    } else {
                        vec!["-c", &cmd.args]
                    })
                    .output()
                    .map_err(PgCliError::Io)?;
                let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                Ok(MetaResult::Output(format!("{stdout}{stderr}")))
            }
            "cd" => {
                let dir = cmd.args.trim();
                if dir.is_empty() {
                    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
                    std::env::set_current_dir(&home).map_err(PgCliError::Io)?;
                } else {
                    std::env::set_current_dir(dir).map_err(PgCliError::Io)?;
                }
                Ok(MetaResult::Ok)
            }

            // -- Introspection extensions ------------------------------------
            "introspect" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\introspect requires a table name".to_string())
                })?;
                let (schema, tbl) = split_schema_table(name);
                Ok(MetaResult::IntrospectTable { schema, name: tbl })
            }
            "ddl" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\ddl requires a table name".to_string())
                })?;
                let (schema, tbl) = split_schema_table(name);
                Ok(MetaResult::DdlTable { schema, name: tbl })
            }
            "explain" => {
                if cmd.args.is_empty() {
                    if self.last_sql.is_empty() {
                        return Err(PgCliError::Config(
                            "\\explain requires a query or a prior SQL statement".to_string(),
                        ));
                    }
                    Ok(MetaResult::Query(format!("EXPLAIN (FORMAT TEXT) {}", self.last_sql)))
                } else {
                    Ok(MetaResult::Query(format!("EXPLAIN (FORMAT TEXT) {}", cmd.args)))
                }
            }
            "watch" => {
                // "\watch diff [secs]" shows a line-diff between consecutive runs.
                let mut args = cmd.args.split_whitespace().peekable();
                let is_diff = args.peek().map(|a| a.eq_ignore_ascii_case("diff")).unwrap_or(false);
                if is_diff { let _ = args.next(); }
                let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2);
                if self.last_sql.is_empty() {
                    return Err(PgCliError::Config(
                        "\\watch: no previous SQL to repeat. Execute a query first.".to_string(),
                    ));
                }
                if is_diff {
                    Ok(MetaResult::WatchDiff { interval_secs: secs })
                } else {
                    Ok(MetaResult::Watch { interval_secs: secs })
                }
            }
            "vacuum" => {
                let target = cmd.args.trim();
                if target.is_empty() {
                    Ok(MetaResult::Query("VACUUM ANALYZE;".to_string()))
                } else {
                    if target.contains(';') {
                        return Err(PgCliError::Config("Invalid table name.".to_string()));
                    }
                    Ok(MetaResult::Query(format!("VACUUM ANALYZE {target};")))
                }
            }
            "analyze" => {
                let target = cmd.args.trim();
                if target.is_empty() {
                    Ok(MetaResult::Query("ANALYZE;".to_string()))
                } else {
                    if target.contains(';') {
                        return Err(PgCliError::Config("Invalid table name.".to_string()));
                    }
                    Ok(MetaResult::Query(format!("ANALYZE {target};")))
                }
            }

            // -- Write last result to file ------------------------------------
            "write" | "W" => {
                let path = cmd.args.trim().trim_matches('"').trim_matches('\'');
                if path.is_empty() {
                    Err(PgCliError::Config("\\write requires a file path".to_string()))
                } else {
                    Ok(MetaResult::WriteResult(path.to_string()))
                }
            }

            // -- Row count display --------------------------------------------
            "rows" | "rowcount" => {
                let sql = if self.last_sql.is_empty() {
                    return Ok(MetaResult::Output("\\rows: no previous query.".to_string()));
                } else {
                    format!("SELECT COUNT(*) AS row_count FROM ({}) _subq", self.last_sql)
                };
                Ok(MetaResult::Query(sql))
            }

            // -- Timing and flow control --------------------------------------
            "sleep" => {
                let secs: f64 = cmd.args.trim().parse().unwrap_or(0.0);
                Ok(MetaResult::Sleep(secs))
            }

            // -- Transaction control meta-commands ---------------------------
            "begin" => Ok(MetaResult::Query("BEGIN;".to_string())),
            "commit" => Ok(MetaResult::Query("COMMIT;".to_string())),
            "rollback" => {
                let to = cmd.args.trim();
                if to.is_empty() {
                    Ok(MetaResult::Query("ROLLBACK;".to_string()))
                } else {
                    if to.contains(';') {
                        return Err(PgCliError::Config("Invalid savepoint name.".to_string()));
                    }
                    Ok(MetaResult::Query(format!("ROLLBACK TO SAVEPOINT {to};")))
                }
            }
            "savepoint" => {
                let name = cmd.args.trim();
                if name.is_empty() {
                    return Err(PgCliError::Config("\\savepoint: requires a name.".to_string()));
                }
                if name.contains(';') {
                    return Err(PgCliError::Config("Invalid savepoint name.".to_string()));
                }
                Ok(MetaResult::Query(format!("SAVEPOINT {name};")))
            }
            "release" => {
                let name = cmd.args.trim();
                if name.is_empty() {
                    return Err(PgCliError::Config("\\release: requires a savepoint name.".to_string()));
                }
                if name.contains(';') {
                    return Err(PgCliError::Config("Invalid savepoint name.".to_string()));
                }
                Ok(MetaResult::Query(format!("RELEASE SAVEPOINT {name};")))
            }

            // -- pgcli-rs analytics extensions -------------------------------
            "size" => Ok(MetaResult::Query(size_query(cmd.first_arg()))),
            "locks" => Ok(MetaResult::Query(locks_query())),
            "activity" => Ok(MetaResult::Query(activity_query())),
            "deps" => Ok(MetaResult::ShowDeps { name: cmd.args.trim().to_string() }),
            "indexes" => Ok(MetaResult::ShowIndexes { name: cmd.args.trim().to_string() }),
            "bloat" => Ok(MetaResult::ShowBloat),
            "stats" => {
                let (schema, name) = split_schema_table(cmd.args.trim());
                Ok(MetaResult::ShowColumnStats { schema, name })
            }
            "partitions" => {
                let (schema, name) = split_schema_table(cmd.args.trim());
                Ok(MetaResult::ShowPartitions { schema, name })
            }
            "sequences" => {
                let pattern = cmd.first_arg().unwrap_or("").to_string();
                Ok(MetaResult::ListSequences { pattern })
            }
            "kill" => {
                let mut toks = cmd.arg_tokens();
                let pid_str = toks.next().unwrap_or("");
                let force = toks.next().map(|t| t.eq_ignore_ascii_case("force")).unwrap_or(false);
                match pid_str.parse::<i32>() {
                    Ok(pid) => Ok(MetaResult::KillBackend { pid, force }),
                    Err(_) => Err(PgCliError::Config(
                        "\\kill: expected integer PID (e.g. \\kill 12345 [force])".to_string(),
                    )),
                }
            }
            "dL" | "dl" => {
                let pattern = cmd.first_arg().unwrap_or("").to_string();
                Ok(MetaResult::ListLanguages { pattern })
            }

            // -- Block 7: error control, UX ----------------------------------
            "warn" => Ok(MetaResult::Warn(self.substitute_vars(&cmd.args))),
            "on_error" | "onerror" => {
                let mode = cmd.first_arg().unwrap_or("stop").to_lowercase();
                if !matches!(mode.as_str(), "stop" | "continue" | "rollback") {
                    return Err(PgCliError::Config(
                        "\\on_error: expected stop, continue, or rollback".to_string(),
                    ));
                }
                Ok(MetaResult::SetOnError(mode))
            }
            "prompt" => {
                let mut toks = cmd.args.splitn(2, char::is_whitespace);
                let first = toks.next().unwrap_or("").trim().to_string();
                let second = toks.next().unwrap_or("").trim().to_string();
                if second.is_empty() {
                    // \prompt VAR  (no prompt text)
                    Ok(MetaResult::Prompt { text: String::new(), var: first })
                } else {
                    // \prompt 'text' VAR  or  \prompt text VAR
                    let text = first.trim_matches('\'').to_string();
                    Ok(MetaResult::Prompt { text, var: second })
                }
            }

            // -- gset: store last query's first row as variables --------------
            "gset" => {
                if self.last_sql.is_empty() {
                    Err(PgCliError::Config(
                        "\\gset: no previous query to store".to_string(),
                    ))
                } else {
                    let prefix = cmd.first_arg().unwrap_or("").to_string();
                    Ok(MetaResult::GSet { prefix })
                }
            }

            // -- gdesc: describe query columns without executing --------------
            // \crosstabview [COL_H [COL_V [COL_DATA]]]: pivot last query result.
            "crosstabview" => {
                let mut toks = cmd.arg_tokens();
                let col_h = toks.next().map(|s| s.to_string()).unwrap_or_default();
                let col_v = toks.next().map(|s| s.to_string()).unwrap_or_default();
                let col_d = toks.next().map(|s| s.to_string()).unwrap_or_default();
                Ok(MetaResult::CrossTabView { col_h, col_v, col_d })
            }
            "gdesc" => {
                if self.last_sql.is_empty() {
                    Err(PgCliError::Config(
                        "\\gdesc: no previous query to describe".to_string(),
                    ))
                } else {
                    Ok(MetaResult::GDesc)
                }
            }

            // -- bench: benchmark last query N times --------------------------
            "bench" => {
                if self.last_sql.is_empty() {
                    return Err(PgCliError::Config(
                        "\\bench: no previous query to benchmark".to_string(),
                    ));
                }
                let count: u32 = cmd.first_arg()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(10);
                if count == 0 || count > 10_000 {
                    return Err(PgCliError::Config(
                        "\\bench: count must be between 1 and 10000".to_string(),
                    ));
                }
                Ok(MetaResult::Bench { count })
            }

            // -- e: open query buffer in $EDITOR -----------------------------
            "e" => Ok(MetaResult::EditAndExecute),

            // -- hist / history: show recent query history --------------------
            "hist" | "history" => {
                let n: usize = cmd.first_arg()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(20);
                let hist_path = dirs::home_dir()
                    .map(|h| h.join(".pgcli_history"))
                    .unwrap_or_else(|| PathBuf::from(".pgcli_history"));
                match std::fs::read_to_string(&hist_path) {
                    Ok(content) => {
                        let lines: Vec<&str> = content.lines().collect();
                        let total = lines.len();
                        let start = total.saturating_sub(n);
                        let out = lines[start..]
                            .iter()
                            .enumerate()
                            .map(|(i, l)| format!("{:4}  {l}", start + i + 1))
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(MetaResult::Output(if out.is_empty() {
                            "History is empty.".to_string()
                        } else {
                            out
                        }))
                    }
                    Err(_) => Ok(MetaResult::Output(
                        "No history file found.".to_string(),
                    )),
                }
            }

            // -- bookmark: save / run / list named queries --------------------
            "bookmark" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\bookmark requires a name".to_string())
                })?;
                if self.last_sql.is_empty() {
                    return Err(PgCliError::Config(
                        "\\bookmark: no previous query to save".to_string(),
                    ));
                }
                self.bookmarks
                    .insert(name.to_string(), self.last_sql.clone());
                bookmarks::save(&self.bookmarks)?;
                Ok(MetaResult::Output(format!(
                    "Saved bookmark '{name}': {}",
                    self.last_sql.chars().take(60).collect::<String>()
                )))
            }

            "bookmarks" => {
                Ok(MetaResult::Output(bookmarks::format_list(&self.bookmarks)))
            }

            "run" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\run requires a bookmark name".to_string())
                })?;
                match self.bookmarks.get(name) {
                    Some(sql) => {
                        let sql = sql.clone();
                        self.last_sql = sql.clone();
                        Ok(MetaResult::Query(sql))
                    }
                    None => Err(PgCliError::Config(format!(
                        "\\run: no bookmark named '{name}'. Use \\bookmarks to list them."
                    ))),
                }
            }

            "delbookmark" | "rmbookmark" => {
                let name = cmd.first_arg().ok_or_else(|| {
                    PgCliError::Config("\\delbookmark requires a name".to_string())
                })?;
                if self.bookmarks.remove(name).is_some() {
                    bookmarks::save(&self.bookmarks)?;
                    Ok(MetaResult::Output(format!("Deleted bookmark '{name}'.")))
                } else {
                    Ok(MetaResult::Output(format!(
                        "No bookmark named '{name}' found."
                    )))
                }
            }

            // -- Help ---------------------------------------------------------
            "?" | "help" => Ok(MetaResult::Output(help_text())),
            "h" => Ok(MetaResult::Output(sql_help(cmd.first_arg()))),

            unknown => Err(PgCliError::Query(format!(
                "invalid command \\{unknown}\nTry \\? for help."
            ))),
        }
    }
}

// --- Schema/table name helper ------------------------------------------------

/// Split `schema.table` into `(schema, table)`.
/// If no `.` is present, returns `("public", name)`.
fn split_schema_table(name: &str) -> (String, String) {
    match name.split_once('.') {
        Some((s, t)) => (s.to_string(), t.to_string()),
        None => ("public".to_string(), name.to_string()),
    }
}

// --- System-catalog query helpers -------------------------------------------

fn list_databases_query() -> String {
    "SELECT datname AS \"Name\", \
     pg_catalog.pg_get_userbyid(datdba) AS \"Owner\", \
     pg_catalog.pg_encoding_to_char(encoding) AS \"Encoding\", \
     datcollate AS \"Collate\", datctype AS \"Ctype\", \
     datacl::text AS \"Access privileges\" \
     FROM pg_catalog.pg_database \
     ORDER BY 1;"
        .to_string()
}

fn list_databases_query_extended() -> String {
    "SELECT datname AS \"Name\", \
     pg_catalog.pg_get_userbyid(datdba) AS \"Owner\", \
     pg_catalog.pg_encoding_to_char(encoding) AS \"Encoding\", \
     datcollate AS \"Collate\", datctype AS \"Ctype\", \
     pg_catalog.pg_size_pretty(pg_catalog.pg_database_size(datname)) AS \"Size\", \
     datacl::text AS \"Access privileges\" \
     FROM pg_catalog.pg_database \
     ORDER BY 1;"
        .to_string()
}

fn list_functions_query_extended(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "p.proname");
    format!(
        "SELECT n.nspname AS \"Schema\", p.proname AS \"Name\", \
         pg_catalog.pg_get_function_result(p.oid) AS \"Result data type\", \
         pg_catalog.pg_get_function_arguments(p.oid) AS \"Argument data types\", \
         CASE p.prokind \
           WHEN 'a' THEN 'agg' \
           WHEN 'w' THEN 'window' \
           WHEN 'p' THEN 'proc' \
           ELSE 'func' END AS \"Type\", \
         CASE p.provolatile \
           WHEN 'i' THEN 'immutable' \
           WHEN 's' THEN 'stable' \
           ELSE 'volatile' END AS \"Volatility\", \
         CASE p.proparallel \
           WHEN 's' THEN 'safe' \
           WHEN 'r' THEN 'restricted' \
           ELSE 'unsafe' END AS \"Parallel\", \
         left(p.prosrc, 120) AS \"Source\" \
         FROM pg_catalog.pg_proc p \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
         {filter} \
         ORDER BY 1, 2;"
    )
}

fn conninfo_query() -> String {
    "SELECT current_database() AS \"Database\", \
     current_user AS \"User\", \
     inet_server_addr()::text AS \"Host\", \
     inet_server_port() AS \"Port\", \
     pg_backend_pid() AS \"Backend PID\", \
     (SELECT COALESCE((SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()), false)) AS \"SSL\", \
     version() AS \"Server version\";"
        .to_string()
}

fn describe_query(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "c.relname");
    format!(
        "SELECT c.relname AS \"Name\", \
         CASE c.relkind \
           WHEN 'r' THEN 'table' \
           WHEN 'v' THEN 'view' \
           WHEN 'm' THEN 'materialized view' \
           WHEN 'i' THEN 'index' \
           WHEN 'S' THEN 'sequence' \
           WHEN 's' THEN 'special' \
           WHEN 'f' THEN 'foreign table' \
           WHEN 'p' THEN 'partitioned table' \
           ELSE c.relkind::text \
         END AS \"Type\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r','v','m','S','f','p') \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1;"
    )
}

fn list_tables_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind = 'r' \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_views_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind = 'v' \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_matviews_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\", \
         CASE WHEN (SELECT count(*) FROM pg_catalog.pg_index i WHERE i.indrelid = c.oid) > 0 \
              THEN 'yes' ELSE 'no' END AS \"Indexed\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind = 'm' \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_event_triggers_query(pattern: Option<&str>) -> String {
    let filter = if let Some(p) = pattern {
        let p = p.replace('\'', "''");
        format!("AND evtname ILIKE '{p}'")
    } else {
        String::new()
    };
    format!(
        "SELECT evtname AS \"Name\", evtevent AS \"Event\", \
         pg_catalog.pg_get_userbyid(evtowner) AS \"Owner\", \
         CASE evtenabled WHEN 'O' THEN 'enabled' WHEN 'D' THEN 'disabled' \
                         WHEN 'R' THEN 'replica' WHEN 'A' THEN 'always' END AS \"Status\", \
         evttags::text AS \"Tags\" \
         FROM pg_catalog.pg_event_trigger \
         WHERE true {filter} \
         ORDER BY evtname;"
    )
}

fn list_types_query_extended(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "t.typname");
    format!(
        "SELECT n.nspname AS \"Schema\", t.typname AS \"Name\", \
         pg_catalog.format_type(t.oid, NULL) AS \"Internal name\", \
         CASE t.typtype WHEN 'b' THEN 'base' WHEN 'c' THEN 'composite' \
                        WHEN 'd' THEN 'domain' WHEN 'e' THEN 'enum' \
                        WHEN 'r' THEN 'range' WHEN 'm' THEN 'multirange' \
                        ELSE t.typtype::text END AS \"Type\", \
         pg_catalog.pg_get_userbyid(t.typowner) AS \"Owner\", \
         COALESCE(pg_catalog.obj_description(t.oid, 'pg_type'), '') AS \"Description\" \
         FROM pg_catalog.pg_type t \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
         WHERE (t.typrelid = 0 OR (SELECT c.relkind = 'c' FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid)) \
         AND NOT EXISTS(SELECT 1 FROM pg_catalog.pg_type el WHERE el.oid = t.typelem AND el.typarray = t.oid) \
         AND n.nspname NOT IN ('pg_catalog','pg_toast','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_ts_configs_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.cfgname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.cfgname AS \"Name\", \
         pg_catalog.pg_get_userbyid(c.cfgowner) AS \"Owner\", \
         pn.nspname AS \"Parser schema\", p.prsname AS \"Parser\" \
         FROM pg_catalog.pg_ts_config c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.cfgnamespace \
         LEFT JOIN pg_catalog.pg_ts_parser p ON p.oid = c.cfgparser \
         LEFT JOIN pg_catalog.pg_namespace pn ON pn.oid = p.prsnamespace \
         WHERE true {filter} \
         ORDER BY 1,2;"
    )
}

fn list_ts_parsers_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "p.prsname");
    format!(
        "SELECT n.nspname AS \"Schema\", p.prsname AS \"Name\" \
         FROM pg_catalog.pg_ts_parser p \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.prsnamespace \
         WHERE true {filter} \
         ORDER BY 1,2;"
    )
}

fn list_ts_dicts_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "d.dictname");
    format!(
        "SELECT n.nspname AS \"Schema\", d.dictname AS \"Name\", \
         pg_catalog.pg_get_userbyid(d.dictowner) AS \"Owner\" \
         FROM pg_catalog.pg_ts_dict d \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = d.dictnamespace \
         WHERE true {filter} \
         ORDER BY 1,2;"
    )
}

fn list_ts_templates_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "t.tmplname");
    format!(
        "SELECT n.nspname AS \"Schema\", t.tmplname AS \"Name\" \
         FROM pg_catalog.pg_ts_template t \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = t.tmplnamespace \
         WHERE true {filter} \
         ORDER BY 1,2;"
    )
}

fn list_domains_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "t.typname");
    format!(
        "SELECT n.nspname AS \"Schema\", t.typname AS \"Name\", \
         pg_catalog.format_type(t.typbasetype, t.typtypmod) AS \"Type\", \
         CASE WHEN t.typnotnull THEN 'not null' ELSE '' END AS \"Nullable\", \
         t.typdefault AS \"Default\", \
         COALESCE(pg_catalog.obj_description(t.oid, 'pg_type'), '') AS \"Description\" \
         FROM pg_catalog.pg_type t \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
         WHERE t.typtype = 'd' \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_collations_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.collname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.collname AS \"Name\", \
         COALESCE(c.collprovider::text, 'libc') AS \"Provider\", \
         c.collcollate AS \"Collate\", c.collctype AS \"Ctype\", \
         COALESCE(pg_catalog.obj_description(c.oid, 'pg_collation'), '') AS \"Description\" \
         FROM pg_catalog.pg_collation c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.collnamespace \
         WHERE n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_partitioned_tables_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\", \
         p.partstrat::text AS \"Strategy\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_catalog.pg_partitioned_table p ON p.partrelid = c.oid \
         WHERE c.relkind = 'p' \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_access_methods_query(pattern: Option<&str>) -> String {
    let filter = if let Some(p) = pattern {
        let p = p.replace('\'', "''");
        format!("AND amname ILIKE '{p}'")
    } else {
        String::new()
    };
    format!(
        "SELECT amname AS \"Name\", \
         CASE amtype WHEN 'i' THEN 'Index' WHEN 't' THEN 'Table' ELSE amtype::text END AS \"Type\", \
         amhandler::text AS \"Handler\" \
         FROM pg_catalog.pg_am \
         WHERE true {filter} \
         ORDER BY 1;"
    )
}

fn list_role_grants_query(pattern: Option<&str>) -> String {
    let filter = if let Some(p) = pattern {
        let p = p.replace('\'', "''");
        format!("AND r.rolname ILIKE '{p}'")
    } else {
        String::new()
    };
    format!(
        "SELECT r.rolname AS \"Role\", \
         r.rolsuper AS \"Superuser\", \
         r.rolinherit AS \"Inherit\", \
         r.rolcreaterole AS \"Create role\", \
         r.rolcreatedb AS \"Create DB\", \
         r.rolcanlogin AS \"Login\", \
         r.rolconnlimit AS \"Conn limit\", \
         r.rolvaliduntil AS \"Expires\" \
         FROM pg_catalog.pg_roles r \
         WHERE true {filter} \
         ORDER BY 1;"
    )
}

fn list_role_memberships_query(pattern: Option<&str>) -> String {
    let filter = if let Some(p) = pattern {
        let p = p.replace('\'', "''");
        format!("AND r.rolname ILIKE '{p}'")
    } else {
        String::new()
    };
    format!(
        "SELECT r.rolname AS \"Role\", \
         m.rolname AS \"Member of\", \
         a.admin_option AS \"Admin option\" \
         FROM pg_catalog.pg_auth_members a \
         JOIN pg_catalog.pg_roles r ON r.oid = a.roleid \
         JOIN pg_catalog.pg_roles m ON m.oid = a.member \
         WHERE true {filter} \
         ORDER BY 1,2;"
    )
}

fn list_indexes_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         c2.relname AS \"Table\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_catalog.pg_index i ON i.indexrelid = c.oid \
         JOIN pg_catalog.pg_class c2 ON c2.oid = i.indrelid \
         WHERE c.relkind = 'i' \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_sequences_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind = 'S' \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_functions_query(pattern: Option<&str>) -> String {
    let filter = schema_pattern_filter(pattern, "n.nspname", "p.proname");
    format!(
        "SELECT n.nspname AS \"Schema\", p.proname AS \"Name\", \
         pg_catalog.pg_get_function_result(p.oid) AS \"Result data type\", \
         pg_catalog.pg_get_function_arguments(p.oid) AS \"Argument data types\", \
         CASE p.prokind \
           WHEN 'a' THEN 'agg' \
           WHEN 'w' THEN 'window' \
           WHEN 'p' THEN 'proc' \
           ELSE 'func' \
         END AS \"Type\" \
         FROM pg_catalog.pg_proc p \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_schemas_query(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "n.nspname");
    format!(
        "SELECT n.nspname AS \"Name\", \
         pg_catalog.pg_get_userbyid(n.nspowner) AS \"Owner\" \
         FROM pg_catalog.pg_namespace n \
         WHERE n.nspname NOT LIKE 'pg_temp_%' \
         AND n.nspname NOT LIKE 'pg_toast%' \
         {filter} \
         ORDER BY 1;"
    )
}

fn list_schemas_extended_query(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "n.nspname");
    format!(
        "SELECT n.nspname AS \"Name\", \
         pg_catalog.pg_get_userbyid(n.nspowner) AS \"Owner\", \
         COALESCE(pg_catalog.obj_description(n.oid, 'pg_namespace'), '') AS \"Description\" \
         FROM pg_catalog.pg_namespace n \
         WHERE n.nspname NOT LIKE 'pg_temp_%' \
         AND n.nspname NOT LIKE 'pg_toast%' \
         {filter} \
         ORDER BY 1;"
    )
}

fn list_casts_query(pattern: Option<&str>) -> String {
    let filter = if let Some(p) = pattern {
        let p = p.replace('\'', "''");
        format!("AND pg_catalog.format_type(st.oid, NULL) ILIKE '{p}'")
    } else {
        String::new()
    };
    format!(
        "SELECT pg_catalog.format_type(st.oid, NULL) AS \"Source type\", \
         pg_catalog.format_type(tt.oid, NULL) AS \"Target type\", \
         CASE c.castfunc WHEN 0 THEN '(binary coercible)' ELSE p.proname END AS \"Function\", \
         CASE c.castcontext \
           WHEN 'e' THEN 'explicit' WHEN 'a' THEN 'assignment' WHEN 'i' THEN 'implicit' \
           ELSE c.castcontext::text END AS \"Implicit?\" \
         FROM pg_catalog.pg_cast c \
         LEFT JOIN pg_catalog.pg_type st ON st.oid = c.castsource \
         LEFT JOIN pg_catalog.pg_type tt ON tt.oid = c.casttarget \
         LEFT JOIN pg_catalog.pg_proc p ON p.oid = c.castfunc \
         WHERE true {filter} \
         ORDER BY 1,2;"
    )
}

fn list_privileges_query(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "c.relname");
    format!(
        "SELECT n.nspname AS \"Schema\", c.relname AS \"Name\", \
         c.relacl::text AS \"Access privileges\" \
         FROM pg_catalog.pg_class c \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r','v','S') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_types_query(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "t.typname");
    format!(
        "SELECT n.nspname AS \"Schema\", t.typname AS \"Name\", \
         pg_catalog.format_type(t.oid, NULL) AS \"Type\" \
         FROM pg_catalog.pg_type t \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = t.typnamespace \
         WHERE (t.typrelid = 0 OR (SELECT c.relkind = 'c' FROM pg_catalog.pg_class c WHERE c.oid = t.typrelid)) \
         AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_type el WHERE el.oid = t.typelem AND el.typarray = t.oid) \
         AND n.nspname NOT IN ('pg_catalog','information_schema') \
         {filter} \
         ORDER BY 1,2;"
    )
}

fn list_extensions_query(pattern: Option<&str>) -> String {
    let filter = pattern_filter(pattern, "e.extname");
    format!(
        "SELECT e.extname AS \"Name\", \
         e.extversion AS \"Version\", \
         n.nspname AS \"Schema\", \
         c.description AS \"Description\" \
         FROM pg_catalog.pg_extension e \
         LEFT JOIN pg_catalog.pg_namespace n ON n.oid = e.extnamespace \
         LEFT JOIN pg_catalog.pg_description c ON c.objoid = e.oid \
         {filter} \
         ORDER BY 1;"
    )
}

fn size_query(pattern: Option<&str>) -> String {
    if let Some(table) = pattern {
        let table = table.replace('\'', "''");
        format!(
            "SELECT \
             pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size('{table}'::regclass)) AS \"Total\", \
             pg_catalog.pg_size_pretty(pg_catalog.pg_relation_size('{table}'::regclass)) AS \"Table\", \
             pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size('{table}'::regclass) - pg_catalog.pg_relation_size('{table}'::regclass)) AS \"Indexes\";"
        )
    } else {
        "SELECT \
         current_database() AS \"Database\", \
         pg_catalog.pg_size_pretty(pg_catalog.pg_database_size(current_database())) AS \"Size\";"
            .to_string()
    }
}

fn locks_query() -> String {
    "SELECT pid, locktype, relation::regclass::text AS relation, mode, granted \
     FROM pg_catalog.pg_locks \
     WHERE relation IS NOT NULL \
     ORDER BY pid, locktype;"
        .to_string()
}

fn activity_query() -> String {
    "SELECT pid, usename AS user, application_name AS app, state, \
     left(query, 80) AS query, \
     now() - query_start AS duration \
     FROM pg_catalog.pg_stat_activity \
     WHERE pid <> pg_backend_pid() \
     ORDER BY query_start;"
        .to_string()
}

/// Build a `WHERE column ILIKE '%pattern%'` filter for a single column.
fn pattern_filter(pattern: Option<&str>, column: &str) -> String {
    match pattern {
        Some(p) if !p.is_empty() => {
            let escaped = p.replace('\'', "''").replace('*', "%").replace('?', "_");
            format!("AND {column} ILIKE '%{escaped}%'")
        }
        _ => String::new(),
    }
}

/// Build a WHERE clause fragment for a schema-qualified name pattern.
fn schema_pattern_filter(pattern: Option<&str>, schema_col: &str, name_col: &str) -> String {
    match pattern {
        None | Some("") => String::new(),
        Some(p) => {
            let p = p.replace('\'', "''");
            if let Some((schema, name)) = p.split_once('.') {
                let sp = schema.replace('*', "%").replace('?', "_");
                let np = name.replace('*', "%").replace('?', "_");
                format!("AND {schema_col} ILIKE '{sp}' AND {name_col} ILIKE '{np}'")
            } else {
                let np = p.replace('*', "%").replace('?', "_");
                format!("AND {name_col} ILIKE '{np}'")
            }
        }
    }
}

fn pset_help() -> String {
    r#"Available \pset options:
  border N           border level (0=none, 1=single rules, 2=double border)
  expanded [on|off]  expanded (vertical) display
  fieldsep STRING    field separator for unaligned output (default |)
  footer [on|off]    show (N rows) footer line
  format FORMAT      output format: table|csv|json|jsonl|tsv|html|unaligned|markdown|latex|asciidoc
  linestyle STYLE    line style: ascii|unicode|old-ascii
  null STRING        string to display for NULL values
  numericlocale [on|off]  use locale-specific number formatting
  pager [on|off]     enable or disable pager for long output
  recordsep STRING   record separator for unaligned output
  timing [on|off]    show query execution time
  title STRING       title printed above each result table
  tuples_only [on|off]   suppress column headers and footer"#
        .to_string()
}

fn help_text() -> String {
    r#"General
  \q                     quit pgcli
  \? [commands]          show this help
  \h [NAME]              help with SQL commands

Connection
  \c [DBNAME [USER [HOST [PORT]]]]
                         connect to new database (- means keep current)
  \conninfo              show current connection info
  \password [USER]       change password hint

Query Buffer
  \p                     show the last executed query
  \r                     reset (clear) the last query
  \e                     edit query buffer in $EDITOR, then execute
  \watch [N]             re-execute last query every N seconds (default 2)
  \gset [PREFIX]         store last query's first row as :variables
  \gdesc                 show column types of last query without executing it

Input / Output
  \i FILE                execute commands from file
  \o [FILE]              send all query results to file (none = stdout)
  \echo TEXT             write text to stdout
  \! [COMMAND]           execute shell command
  \cd [DIR]              change working directory

Formatting
  \x [on|off]            toggle expanded display mode
  \t [on|off]            show only rows (hint: use --tuples-only flag)
  \timing [on|off]       toggle query timing (default: on)
  \a                     toggle aligned/unaligned (hint: use --format)
  \format FORMAT         switch output format (table|csv|json|jsonl|tsv|html)
  \theme [dark|light|none]
                         change color theme for SQL highlighting
  \pset NAME [VALUE]     set table output option (limited support)

Informational
  \d [PATTERN]           list tables, views, sequences - or describe if exact name
  \dt [PATTERN]          list tables
  \dv [PATTERN]          list views
  \di [PATTERN]          list indexes
  \ds [PATTERN]          list sequences
  \df [PATTERN]          list functions
  \dn [PATTERN]          list schemas
  \du [PATTERN]          list roles/users (\dg is an alias)
  \dp [PATTERN]          list access privileges (\z is an alias)
  \dT [PATTERN]          list data types
  \dx [PATTERN]          list installed extensions
  \l                     list databases (\list is an alias)

Variables
  \set [NAME [VALUE]]    set variable, or list all when called without args
  \unset NAME            delete variable
  \echo TEXT             print text (substitutes :variables)

History
  \hist [N]              show last N queries from history (default 20)
  \history [N]           alias for \hist

Bookmarks (saved to ~/.pgcli_bookmarks.toml)
  \bookmark NAME         save current query as named bookmark
  \bookmarks             list all saved bookmarks
  \run NAME              execute a saved bookmark
  \delbookmark NAME      delete a named bookmark

pgcli extensions
  \introspect TABLE      describe table columns, constraints, and indexes
  \ddl TABLE             show CREATE TABLE DDL for a table
  \explain [QUERY]       run EXPLAIN on query (or last query if omitted)
  \bench [N]             run last query N times and show timing statistics
  \size [TABLE]          show disk size (database if no table given)
  \locks                 show current lock table
  \activity              show pg_stat_activity
  \vacuum                run VACUUM ANALYZE"#
        .to_string()
}

fn sql_help(command: Option<&str>) -> String {
    match command {
        Some(cmd) => format!("SQL help for '{cmd}': see https://www.postgresql.org/docs/current/sql-{}.html",
            cmd.to_lowercase().replace(' ', "-")),
        None => "Use \\h COMMAND for help on a specific SQL command.\nCommon commands: SELECT, INSERT, UPDATE, DELETE, CREATE TABLE, ALTER TABLE, DROP TABLE, EXPLAIN, VACUUM, COPY".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quit_command() {
        let cmd = MetaCommand::parse(r"\q").unwrap();
        assert_eq!(cmd.name, "q");
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn parse_with_args() {
        let cmd = MetaCommand::parse(r"\d mytable").unwrap();
        assert_eq!(cmd.name, "d");
        assert_eq!(cmd.args, "mytable");
    }

    #[test]
    fn parse_no_backslash_returns_none() {
        assert!(MetaCommand::parse("SELECT 1").is_none());
    }

    #[test]
    fn dispatch_quit() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\q").unwrap();
        assert_eq!(d.dispatch(&cmd).unwrap(), MetaResult::Quit);
    }

    #[test]
    fn dispatch_list_returns_query() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\l").unwrap();
        assert!(matches!(d.dispatch(&cmd).unwrap(), MetaResult::Query(_)));
    }

    #[test]
    fn dispatch_x_toggles() {
        let mut d = MetaCommandDispatcher::new();
        assert!(!d.expanded);
        let cmd = MetaCommand::parse(r"\x").unwrap();
        d.dispatch(&cmd).unwrap();
        assert!(d.expanded);
        d.dispatch(&cmd).unwrap();
        assert!(!d.expanded);
    }

    #[test]
    fn dispatch_unknown_returns_error() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\zzz").unwrap();
        assert!(d.dispatch(&cmd).is_err());
    }

    #[test]
    fn dispatch_set_stores_variable() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\set FOO bar").unwrap();
        d.dispatch(&cmd).unwrap();
        assert_eq!(d.variables.get("FOO").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn substitute_vars_replaces_tokens() {
        let mut d = MetaCommandDispatcher::new();
        d.variables
            .insert("schema".to_string(), "public".to_string());
        d.variables.insert("tbl".to_string(), "users".to_string());
        let result = d.substitute_vars("SELECT * FROM :schema.:tbl WHERE id = :id");
        assert_eq!(result, "SELECT * FROM public.users WHERE id = :id");
    }

    #[test]
    fn substitute_vars_noop_when_empty() {
        let d = MetaCommandDispatcher::new();
        let sql = "SELECT 1";
        assert_eq!(d.substitute_vars(sql), sql);
    }

    #[test]
    fn dispatch_d_exact_name_returns_introspect() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\d users").unwrap();
        assert!(matches!(
            d.dispatch(&cmd).unwrap(),
            MetaResult::IntrospectTable { .. }
        ));
    }

    #[test]
    fn dispatch_d_pattern_returns_query() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\d us*").unwrap();
        assert!(matches!(d.dispatch(&cmd).unwrap(), MetaResult::Query(_)));
    }

    #[test]
    fn dispatch_c_parses_args() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\c mydb alice").unwrap();
        let result = d.dispatch(&cmd).unwrap();
        assert_eq!(
            result,
            MetaResult::Reconnect {
                dbname: Some("mydb".to_string()),
                user: Some("alice".to_string()),
                host: None,
                port: None,
            }
        );
    }

    #[test]
    fn split_schema_table_with_dot() {
        let (s, t) = split_schema_table("myschema.mytable");
        assert_eq!(s, "myschema");
        assert_eq!(t, "mytable");
    }

    #[test]
    fn split_schema_table_no_dot() {
        let (s, t) = split_schema_table("mytable");
        assert_eq!(s, "public");
        assert_eq!(t, "mytable");
    }

    #[test]
    fn dispatch_theme_changes_theme() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\theme light").unwrap();
        d.dispatch(&cmd).unwrap();
        assert_eq!(d.theme, "light");
    }

    #[test]
    fn dispatch_p_shows_last_sql() {
        let mut d = MetaCommandDispatcher::new();
        d.last_sql = "SELECT 1".to_string();
        let cmd = MetaCommand::parse(r"\p").unwrap();
        assert_eq!(
            d.dispatch(&cmd).unwrap(),
            MetaResult::Output("SELECT 1".to_string())
        );
    }

    #[test]
    fn dispatch_describe_same_as_d() {
        let mut d = MetaCommandDispatcher::new();
        let cmd_d = MetaCommand::parse(r"\d employees").unwrap();
        let cmd_desc = MetaCommand::parse(r"\describe employees").unwrap();
        let r1 = d.dispatch(&cmd_d).unwrap();
        let r2 = d.dispatch(&cmd_desc).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn dispatch_include_verbose_returns_path() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\i+ /tmp/test.sql").unwrap();
        match d.dispatch(&cmd).unwrap() {
            MetaResult::ExecuteFileVerbose(p) => assert_eq!(p.to_str().unwrap(), "/tmp/test.sql"),
            other => panic!("expected ExecuteFileVerbose, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_lo_list_returns_query() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\lo_list").unwrap();
        assert!(matches!(d.dispatch(&cmd).unwrap(), MetaResult::Query(_)));
    }

    #[test]
    fn dispatch_sleep_parses_value() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\sleep 1.5").unwrap();
        match d.dispatch(&cmd).unwrap() {
            MetaResult::Sleep(secs) => assert!((secs - 1.5).abs() < 0.001),
            other => panic!("expected Sleep, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_da_returns_query() {
        let mut d = MetaCommandDispatcher::new();
        let cmd = MetaCommand::parse(r"\dA").unwrap();
        assert!(matches!(d.dispatch(&cmd).unwrap(), MetaResult::Query(_)));
    }
}
