/// SQL script runner with psql variable substitution and conditional execution.
///
/// Supports `\set VAR VALUE`, `:VAR` substitution, and `\if` / `\elif` /
/// `\else` / `\endif` conditional blocks (psql 10+ syntax).
use std::collections::HashMap;
use std::path::Path;

use tokio_postgres::Client;
use tracing::info;

use crate::error::{PgCliError, Result};
use crate::executor::query::QueryExecutor;
use crate::protocol::messages::QueryResult;

/// Runs a SQL script file with variable substitution and conditional blocks.
pub struct ScriptRunner {
    /// Variable store: `\set` bindings and `:var` substitution targets.
    variables: HashMap<String, String>,
    /// Nesting stack for `\if` / `\elif` / `\else` / `\endif`.
    if_stack: Vec<IfState>,
    /// When `true`, print each statement before executing and wait for user confirmation.
    pub single_step: bool,
    /// Error handling mode: `"stop"` (default), `"continue"`, or `"rollback"`.
    pub on_error_mode: String,
}

/// State for a single level of `\if` nesting.
#[derive(Debug, Clone)]
struct IfState {
    /// Whether the current block is active (executing).
    active: bool,
    /// Whether any branch in this `\if` has already been taken.
    branch_taken: bool,
}

impl ScriptRunner {
    /// Create a new `ScriptRunner` with an empty variable store.
    pub fn new() -> Self {
        Self {
            variables: HashMap::new(),
            if_stack: Vec::new(),
            single_step: false,
            on_error_mode: "stop".to_string(),
        }
    }

    /// Create a `ScriptRunner` pre-populated with the given variables.
    pub fn with_variables(variables: HashMap<String, String>) -> Self {
        Self {
            variables,
            if_stack: Vec::new(),
            single_step: false,
            on_error_mode: "stop".to_string(),
        }
    }

    /// Set a variable binding.
    pub fn set_variable(&mut self, name: &str, value: &str) {
        self.variables.insert(name.to_string(), value.to_string());
    }

    /// Execute a SQL script file against `client`.
    ///
    /// Processes each line for meta-directives (`\set`, `\if`, etc.) and
    /// collects statements terminated by `;` for execution.
    ///
    /// If `single_transaction` is `true`, all statements are wrapped in
    /// `BEGIN` / `COMMIT` (rollback on any error).
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if the file cannot be read,
    /// or `PgCliError::Query` on any statement failure.
    pub async fn run_file(
        &mut self,
        client: &Client,
        path: &Path,
        single_transaction: bool,
    ) -> Result<Vec<QueryResult>> {
        let source = std::fs::read_to_string(path).map_err(PgCliError::Io)?;
        self.run_source(client, &source, single_transaction).await
    }

    /// Execute a SQL source string with the same processing as `run_file`.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on statement failure.
    pub async fn run_source(
        &mut self,
        client: &Client,
        source: &str,
        single_transaction: bool,
    ) -> Result<Vec<QueryResult>> {
        if single_transaction {
            client
                .execute("BEGIN", &[])
                .await
                .map_err(|e| PgCliError::Query(e.to_string()))?;
        }

        let result = self.execute_source(client, source).await;

        if single_transaction {
            match &result {
                Ok(_) => {
                    client
                        .execute("COMMIT", &[])
                        .await
                        .map_err(|e| PgCliError::Query(e.to_string()))?;
                }
                Err(_) => {
                    let _ = client.execute("ROLLBACK", &[]).await;
                }
            }
        }

        result
    }

    /// Internal: process source text line by line, accumulating and executing statements.
    async fn execute_source(&mut self, client: &Client, source: &str) -> Result<Vec<QueryResult>> {
        let mut results = Vec::new();
        let mut stmt_buf = String::new();
        let mut stmt_index = 0usize;
        let mut last_result: Option<QueryResult> = None;
        // Track whether we are inside a dollar-quoted block ($$...$$).
        // When Some, contains the tag (e.g. "" for $$, "body" for $body$).
        let mut dollar_tag: Option<String> = None;

        for (line_no, raw_line) in source.lines().enumerate() {
            let line = raw_line.trim_end();

            // Handle meta-directives that affect execution flow.
            if let Some(directive) = line.trim_start().strip_prefix('\\') {
                // Flush any pending statement first.
                if !stmt_buf.trim().is_empty() {
                    let sql = self.substitute(&stmt_buf);
                    info!("Statement {stmt_index}: {}", sql.chars().take(60).collect::<String>());
                    if self.single_step && !single_step_confirm(&sql)? {
                        stmt_buf.clear();
                        stmt_index += 1;
                        continue;
                    }
                    match QueryExecutor::execute(client, &sql).await {
                        Ok(res) => {
                            last_result = Some(res.clone());
                            results.push(res);
                        }
                        Err(e) => {
                            stmt_buf.clear();
                            stmt_index += 1;
                            match self.on_error_mode.as_str() {
                                "continue" => {
                                    eprintln!("ERROR (continuing): {e}");
                                    continue;
                                }
                                _ => return Err(e),
                            }
                        }
                    }
                    stmt_buf.clear();
                    stmt_index += 1;
                }

                self.handle_directive(directive, line_no + 1, &last_result)?;
                continue;
            }

            // Skip lines inside inactive conditional blocks.
            if !self.is_active() {
                continue;
            }

            // Accumulate SQL.
            if !stmt_buf.is_empty() {
                stmt_buf.push('\n');
            }
            stmt_buf.push_str(line);

            // Track dollar-quote state for this line so we do not split on
            // semicolons that appear inside $$ function bodies.
            update_dollar_tag(line, &mut dollar_tag);

            // Execute on semicolon termination — but only when outside a dollar-quoted block.
            // Strip an inline comment from the end of the line before checking,
            // so that `SELECT 1;  -- comment` correctly triggers a flush.
            let line_for_check = {
                let s = line.trim_end();
                if let Some(pos) = s.find("; --") {
                    s[..pos + 1].trim_end()
                } else {
                    s
                }
            };
            if dollar_tag.is_none() && line_for_check.ends_with(';')
            {
                let sql = self.substitute(&stmt_buf);
                let trimmed = sql.trim().to_string();
                if !trimmed.is_empty() {
                    let preview: String = trimmed.chars().take(60).collect();
                    info!(
                        "Statement {stmt_index} (line {}): {}...",
                        line_no + 1,
                        preview
                    );
                    if self.single_step && !single_step_confirm(&trimmed)? {
                        stmt_buf.clear();
                        stmt_index += 1;
                        continue;
                    }
                    match QueryExecutor::execute(client, &trimmed).await {
                        Ok(res) => {
                            last_result = Some(res.clone());
                            results.push(res);
                        }
                        Err(e) => {
                            stmt_buf.clear();
                            stmt_index += 1;
                            match self.on_error_mode.as_str() {
                                "continue" => {
                                    eprintln!("ERROR (continuing): {e}");
                                    continue;
                                }
                                _ => return Err(e),
                            }
                        }
                    }
                    stmt_index += 1;
                }
                stmt_buf.clear();
            }
        }

        // Execute any trailing statement without semicolon.
        let remaining = self.substitute(&stmt_buf);
        let remaining = remaining.trim().to_string();
        if !remaining.is_empty() && (!self.single_step || single_step_confirm(&remaining)?) {
            let res = QueryExecutor::execute(client, &remaining).await?;
            let _ = last_result;
            results.push(res);
        }

        Ok(results)
    }

    /// Process a backslash directive line (without the leading `\`).
    fn handle_directive(
        &mut self,
        directive: &str,
        line_no: usize,
        last_result: &Option<QueryResult>,
    ) -> Result<()> {
        let mut parts = directive.splitn(3, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let rest: String = parts.collect::<Vec<_>>().join(" ");

        match cmd {
            "set" => {
                let mut toks = rest.splitn(2, char::is_whitespace);
                if let Some(name) = toks.next() {
                    let value = toks.next().unwrap_or("").trim().to_string();
                    self.variables.insert(name.trim().to_string(), value);
                }
            }
            "unset" => {
                if let Some(name) = rest.split_whitespace().next() {
                    self.variables.remove(name);
                }
            }
            // Store the first row of the last query result as variables.
            "gset" => {
                let prefix = rest.trim();
                if let Some(result) = last_result {
                    if let Some(row) = result.rows.first() {
                        for (col, val) in result.columns.iter().zip(row.values.iter()) {
                            let key = format!("{prefix}{}", col.name);
                            self.variables.insert(key, val.to_string());
                        }
                    }
                }
            }
            // Print text (with variable substitution) to stdout.
            "echo" | "print" => {
                let msg = self.substitute(rest.trim());
                println!("{msg}");
            }
            "qecho" => {
                let msg = self.substitute(rest.trim());
                println!("{msg}");
            }
            "warn" => {
                let msg = self.substitute(rest.trim());
                eprintln!("WARNING: {msg}");
            }
            "!" => {
                // Shell escape — output goes to stdout; errors not fatal.
                let shell_cmd = self.substitute(rest.trim());
                #[cfg(unix)]
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&shell_cmd)
                    .status();
                #[cfg(windows)]
                let _ = std::process::Command::new("cmd")
                    .args(["/C", &shell_cmd])
                    .status();
            }
            "on_error" | "onerror" => {
                let mode = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("stop")
                    .to_lowercase();
                self.on_error_mode = mode;
            }
            "if" => {
                let cond = self.evaluate_condition(rest.trim());
                self.if_stack.push(IfState {
                    active: cond,
                    branch_taken: cond,
                });
            }
            "elif" => {
                let cond = self.evaluate_condition_static(rest.trim());
                if let Some(state) = self.if_stack.last_mut() {
                    if !state.branch_taken && cond {
                        state.active = true;
                        state.branch_taken = true;
                    } else {
                        state.active = false;
                    }
                } else {
                    return Err(PgCliError::Config(format!(
                        "\\elif without \\if at line {line_no}"
                    )));
                }
            }
            "else" => {
                if let Some(state) = self.if_stack.last_mut() {
                    state.active = !state.branch_taken;
                } else {
                    return Err(PgCliError::Config(format!(
                        "\\else without \\if at line {line_no}"
                    )));
                }
            }
            "endif" => {
                self.if_stack.pop().ok_or_else(|| {
                    PgCliError::Config(format!("\\endif without \\if at line {line_no}"))
                })?;
            }
            // Ignore other meta-commands in the script context.
            _ => {}
        }

        Ok(())
    }

    /// Returns `true` when the current execution context is active.
    fn is_active(&self) -> bool {
        self.if_stack.iter().all(|s| s.active)
    }

    /// Evaluate a `\if` condition expression.
    ///
    /// Supports: `:VAR` (truthy if non-empty), `'literal'` equality checks.
    fn evaluate_condition(&self, expr: &str) -> bool {
        self.evaluate_condition_static(expr)
    }

    /// Condition evaluation without `&self` mutability (used for `\elif`).
    ///
    /// Supports:
    /// - Boolean literals: `true`, `false`, `1`, `0`
    /// - Variable truthiness: `:VAR`
    /// - Comparisons: `LHS OP RHS` where OP is `=`, `!=`, `<`, `>`, `<=`, `>=`
    ///   LHS/RHS may be `:VAR` references or quoted/unquoted strings.
    fn evaluate_condition_static(&self, expr: &str) -> bool {
        let expr = expr.trim();

        // Try comparison: detect `LHS OP RHS` pattern.
        for op in &["!=", "<=", ">=", "=", "<", ">"] {
            if let Some(pos) = expr.find(op) {
                // Make sure not to match inside a variable name.
                let lhs_raw = expr[..pos].trim();
                let rhs_raw = expr[pos + op.len()..].trim();
                let lhs = self.resolve_value(lhs_raw);
                let rhs = self.resolve_value(rhs_raw);
                return match *op {
                    "=" => lhs == rhs,
                    "!=" => lhs != rhs,
                    "<" => lhs < rhs,
                    ">" => lhs > rhs,
                    "<=" => lhs <= rhs,
                    ">=" => lhs >= rhs,
                    _ => false,
                };
            }
        }

        // :VAR — truthy if variable is set and non-empty.
        if let Some(name) = expr.strip_prefix(':') {
            return self
                .variables
                .get(name)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        }

        // Boolean literals.
        match expr.to_uppercase().as_str() {
            "'TRUE'" | "TRUE" | "1" => true,
            "'FALSE'" | "FALSE" | "0" | "" => false,
            _ => false,
        }
    }

    /// Resolve a condition operand: dereference `:VAR`, strip quotes, or return as-is.
    fn resolve_value<'a>(&'a self, raw: &'a str) -> String {
        if let Some(name) = raw.strip_prefix(':') {
            return self.variables.get(name).cloned().unwrap_or_default();
        }
        // Strip surrounding single or double quotes.
        if (raw.starts_with('\'') && raw.ends_with('\''))
            || (raw.starts_with('"') && raw.ends_with('"'))
        {
            return raw[1..raw.len() - 1].to_string();
        }
        raw.to_string()
    }

    /// Substitute `:variable` references in `sql` with their current values.
    fn substitute(&self, sql: &str) -> String {
        let mut result = sql.to_string();
        for (name, value) in &self.variables {
            result = result.replace(&format!(":{name}"), value);
        }
        result
    }
}

/// Update `dollar_tag` state by scanning `line` for `$tag$` open/close markers.
///
/// Called once per line so the runner knows whether a `;` on that line is
/// inside a dollar-quoted block and must not trigger statement flush.
fn update_dollar_tag(line: &str, dollar_tag: &mut Option<String>) {
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '$' {
            continue;
        }
        // Attempt to read a dollar-quote tag: $[A-Za-z0-9_]*$
        let mut tag = String::new();
        let mut valid = false;
        let mut tmp_chars = chars.clone();
        while let Some(&next) = tmp_chars.peek() {
            if next == '$' {
                tmp_chars.next();
                valid = true;
                break;
            } else if next.is_ascii_alphanumeric() || next == '_' {
                tag.push(next);
                tmp_chars.next();
            } else {
                break;
            }
        }
        if !valid {
            continue;
        }
        // Consume the tag characters from the main iterator.
        for _ in 0..tag.len() {
            chars.next();
        }
        chars.next(); // consume the closing '$' of the tag

        if let Some(ref open_tag) = dollar_tag.clone() {
            if tag == *open_tag {
                *dollar_tag = None; // found matching close tag
            }
        } else {
            *dollar_tag = Some(tag); // entering a dollar-quoted block
        }
    }
}

impl Default for ScriptRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Print the statement and ask the user to confirm before executing.
///
/// Returns `true` if execution should proceed, `false` to skip the statement.
/// A `q` answer aborts the script with `PgCliError::Interrupted`.
fn single_step_confirm(sql: &str) -> Result<bool> {
    use std::io::{BufRead, Write};
    eprintln!("***(Single step mode: verify command)*****************************");
    eprintln!("{sql}");
    eprint!("***(press return to proceed or enter x and return to cancel)*****: ");
    std::io::stderr().flush().ok();

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(PgCliError::Io)?;

    let answer = line.trim();
    if answer.eq_ignore_ascii_case("q") || answer.eq_ignore_ascii_case("quit") {
        return Err(PgCliError::Interrupted);
    }
    Ok(!answer.eq_ignore_ascii_case("x") && !answer.eq_ignore_ascii_case("n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_replaces_vars() {
        let mut runner = ScriptRunner::new();
        runner.set_variable("myvar", "42");
        assert_eq!(runner.substitute("SELECT :myvar"), "SELECT 42");
    }

    #[test]
    fn substitute_no_match_unchanged() {
        let runner = ScriptRunner::new();
        assert_eq!(runner.substitute("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn condition_true_literal() {
        let runner = ScriptRunner::new();
        assert!(runner.evaluate_condition("TRUE"));
        assert!(!runner.evaluate_condition("FALSE"));
    }

    #[test]
    fn condition_variable_set() {
        let mut runner = ScriptRunner::new();
        runner.set_variable("DEBUG", "1");
        assert!(runner.evaluate_condition(":DEBUG"));
        assert!(!runner.evaluate_condition(":MISSING"));
    }

    #[test]
    fn is_active_no_stack() {
        let runner = ScriptRunner::new();
        assert!(runner.is_active());
    }

    #[test]
    fn is_active_with_false_frame() {
        let mut runner = ScriptRunner::new();
        runner.if_stack.push(IfState {
            active: false,
            branch_taken: false,
        });
        assert!(!runner.is_active());
    }

    #[test]
    fn handle_set_directive() {
        let mut runner = ScriptRunner::new();
        runner.handle_directive("set FOO bar", 1, &None).unwrap();
        assert_eq!(runner.variables.get("FOO").map(|s| s.as_str()), Some("bar"));
    }

    #[test]
    fn handle_if_endif() {
        let mut runner = ScriptRunner::new();
        runner.handle_directive("if TRUE", 1, &None).unwrap();
        assert!(runner.is_active());
        runner.handle_directive("endif", 2, &None).unwrap();
        assert!(runner.is_active());
    }

    #[test]
    fn handle_endif_without_if_errors() {
        let mut runner = ScriptRunner::new();
        assert!(runner.handle_directive("endif", 1, &None).is_err());
    }

    #[test]
    fn handle_gset_stores_variables() {
        use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
        let mut runner = ScriptRunner::new();
        let result = QueryResult {
            columns: vec![
                Column {
                    name: "db".to_string(),
                    type_name: "text".to_string(),
                    type_oid: 25,
                    nullable: true,
                },
                Column {
                    name: "usr".to_string(),
                    type_name: "text".to_string(),
                    type_oid: 25,
                    nullable: true,
                },
            ],
            rows: vec![Row {
                values: vec![
                    CellValue::Text("mydb".to_string()),
                    CellValue::Text("alice".to_string()),
                ],
            }],
            affected_rows: None,
            command_tag: "SELECT 1".to_string(),
            duration_ms: 0,
        };
        runner.handle_directive("gset", 1, &Some(result)).unwrap();
        assert_eq!(runner.variables.get("db").map(|s| s.as_str()), Some("mydb"));
        assert_eq!(
            runner.variables.get("usr").map(|s| s.as_str()),
            Some("alice")
        );
    }

    #[test]
    fn handle_gset_with_prefix() {
        use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
        let mut runner = ScriptRunner::new();
        let result = QueryResult {
            columns: vec![Column {
                name: "id".to_string(),
                type_name: "int4".to_string(),
                type_oid: 23,
                nullable: true,
            }],
            rows: vec![Row {
                values: vec![CellValue::Int4(42)],
            }],
            affected_rows: None,
            command_tag: "SELECT 1".to_string(),
            duration_ms: 0,
        };
        runner
            .handle_directive("gset my_", 1, &Some(result))
            .unwrap();
        assert_eq!(
            runner.variables.get("my_id").map(|s| s.as_str()),
            Some("42")
        );
    }

    #[test]
    fn condition_equality_comparison() {
        let mut runner = ScriptRunner::new();
        runner.set_variable("TIER", "gold");
        assert!(runner.evaluate_condition(":TIER = 'gold'"));
        assert!(!runner.evaluate_condition(":TIER = 'silver'"));
    }

    #[test]
    fn condition_inequality_comparison() {
        let mut runner = ScriptRunner::new();
        runner.set_variable("X", "5");
        assert!(runner.evaluate_condition(":X != '3'"));
        assert!(!runner.evaluate_condition(":X != '5'"));
    }

    #[test]
    fn condition_literal_comparison() {
        let runner = ScriptRunner::new();
        assert!(runner.evaluate_condition("'hello' = 'hello'"));
        assert!(!runner.evaluate_condition("'hello' = 'world'"));
    }

    #[test]
    fn resolve_value_strips_quotes() {
        let runner = ScriptRunner::new();
        assert_eq!(runner.resolve_value("'hello'"), "hello");
        assert_eq!(runner.resolve_value("\"world\""), "world");
        assert_eq!(runner.resolve_value("bare"), "bare");
    }

    #[test]
    fn resolve_value_var_lookup() {
        let mut runner = ScriptRunner::new();
        runner.set_variable("K", "found");
        assert_eq!(runner.resolve_value(":K"), "found");
        assert_eq!(runner.resolve_value(":MISSING"), "");
    }
}
