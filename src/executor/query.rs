/// Single-statement and batch SQL execution.
use std::time::Instant;

use futures_util::TryStreamExt;
use tokio_postgres::Client;
use tracing::debug;

use crate::error::{PgCliError, Result};
use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
use crate::protocol::types::{extract_cell, oid_to_name};

/// Executes SQL statements against a live PostgreSQL connection.
pub struct QueryExecutor;

impl QueryExecutor {
    /// Execute a single SQL statement and return a structured `QueryResult`.
    ///
    /// Uses the extended query protocol (`prepare` + `query_raw`) so that
    /// `rows_affected()` is available for DML statements (INSERT, UPDATE, DELETE).
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on statement execution failure.
    pub async fn execute(client: &Client, sql: &str) -> Result<QueryResult> {
        debug!("Executing: {sql}");
        let start = Instant::now();

        // The extended query protocol (prepare) does not accept a trailing semicolon
        // or multiple commands.  Strip the semicolon so that scripts executed via -f
        // behave identically to interactive use.
        let sql_clean = sql.trim_end().trim_end_matches(';').trim_end();

        // DDL statements and utility commands (CREATE, ALTER, DROP, TRUNCATE,
        // COMMENT, GRANT, REVOKE, VACUUM, ANALYZE, REFRESH, SET, RESET,
        // NOTIFY, LISTEN, UNLISTEN) use client.execute() to avoid extended-query-
        // protocol issues with DDL like CREATE MATERIALIZED VIEW WITH DATA which
        // materializes rows internally but does not return them to the client.
        // SELECT and DML with RETURNING use prepare+query_raw to get typed rows.
        let first_word = sql_clean
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        let is_returning_select = first_word == "SELECT"
            || first_word == "TABLE"
            || first_word == "VALUES"
            || first_word == "WITH"
            || first_word == "EXPLAIN"  // EXPLAIN always returns rows
            || sql_clean.to_ascii_uppercase().contains("RETURNING");

        if !is_returning_select {
            return Self::execute_dml(client, sql_clean).await;
        }

        let statement = client
            .prepare(sql_clean)
            .await
            .map_err(|e| PgCliError::Query(format_db_error(&e, sql_clean)))?;

        // Use an empty typed iterator; Empty<i32> satisfies ExactSizeIterator
        // and i32 satisfies BorrowToSql. The array is empty so no params are encoded.
        let row_stream = client
            .query_raw(&statement, std::iter::empty::<i32>())
            .await
            .map_err(|e| PgCliError::Query(format_db_error(&e, sql_clean)))?;

        futures_util::pin_mut!(row_stream);

        let mut pg_rows = Vec::new();
        while let Some(row) = row_stream
            .try_next()
            .await
            .map_err(|e| PgCliError::Query(format_db_error(&e, sql_clean)))?
        {
            pg_rows.push(row);
        }
        let affected_rows = row_stream.rows_affected();

        let duration_ms = start.elapsed().as_millis() as u64;

        let columns: Vec<Column> = statement
            .columns()
            .iter()
            .map(|c| Column {
                name: c.name().to_string(),
                type_name: oid_to_name(c.type_().oid()).to_string(),
                type_oid: c.type_().oid(),
                nullable: true, // nullability requires catalog lookup; default true
            })
            .collect();

        let rows: Vec<Row> = pg_rows
            .iter()
            .map(|pg_row| {
                let values: Vec<CellValue> = (0..columns.len())
                    .map(|i| extract_cell(pg_row, i))
                    .collect();
                Row { values }
            })
            .collect();

        let row_count = rows.len();
        let verb = sql_clean
            .split_whitespace()
            .next()
            .unwrap_or("OK")
            .to_ascii_uppercase();
        let command_tag = if columns.is_empty() {
            // No result columns: DDL or DML without RETURNING.
            // Only DML verbs (INSERT/UPDATE/DELETE/COPY) carry a meaningful affected-row
            // count in the tag. DDL and utility commands (CREATE, DROP, VACUUM, etc.)
            // return just the verb even when rows_affected() is Some(0).
            let is_dml = matches!(verb.as_str(), "INSERT" | "UPDATE" | "DELETE" | "COPY");
            if is_dml {
                match affected_rows {
                    Some(n) => format!("{verb} {n}"),
                    None => verb,
                }
            } else {
                verb
            }
        } else if matches!(verb.as_str(), "INSERT" | "UPDATE" | "DELETE") {
            // DML with RETURNING: tag uses the DML verb, not SELECT.
            format!("{verb} {row_count}")
        } else {
            format!("SELECT {row_count}")
        };

        Ok(QueryResult {
            columns,
            rows,
            affected_rows,
            command_tag,
            duration_ms,
        })
    }

    /// Execute a SQL string that may contain multiple `;`-separated statements.
    ///
    /// Each statement is executed individually and all results are collected.
    /// Execution stops on the first error.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on the first statement that fails.
    pub async fn execute_batch(client: &Client, sql: &str) -> Result<Vec<QueryResult>> {
        let statements = split_statements(sql);
        let mut results = Vec::with_capacity(statements.len());
        for stmt in statements {
            let trimmed = stmt.trim();
            if trimmed.is_empty() {
                continue;
            }
            results.push(Self::execute(client, trimmed).await?);
        }
        Ok(results)
    }

    /// Execute a non-SELECT statement (INSERT, UPDATE, DELETE, DDL) and return
    /// the affected-row count if reported by the server.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on failure.
    pub async fn execute_dml(client: &Client, sql: &str) -> Result<QueryResult> {
        debug!("Executing DML: {sql}");
        let start = Instant::now();

        let affected = client
            .execute(sql, &[])
            .await
            .map_err(|e| PgCliError::Query(format_db_error(&e, sql)))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let verb = sql
            .split_whitespace()
            .next()
            .unwrap_or("DML")
            .to_ascii_uppercase();

        // Only DML verbs carry a meaningful row count in the command tag.
        // Utility commands (VACUUM, ANALYZE, CREATE, DROP, etc.) use just the verb.
        let is_dml = matches!(verb.as_str(), "INSERT" | "UPDATE" | "DELETE" | "COPY");
        let command_tag = if is_dml {
            format!("{verb} {affected}")
        } else {
            verb
        };

        Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: Some(affected),
            command_tag,
            duration_ms,
        })
    }
}

/// Split a SQL string on semicolons, respecting string literals, comments,
/// and dollar-quoted blocks (`$$…$$` or `$tag$…$tag$`).
///
/// Dollar-quoting is used extensively in PostgreSQL function bodies.
/// A semicolon inside a dollar-quoted block does NOT split statements.
fn split_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut in_single_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    // Some(tag) when inside a dollar-quoted block; tag is the text between the $…$.
    let mut dollar_tag: Option<String> = None;
    let mut prev = '\0';

    while let Some(ch) = chars.next() {
        // -- Dollar-quoted block ---------------------------------------------
        // Accumulate every character; exit when current ends with the close tag.
        if dollar_tag.is_some() {
            current.push(ch);
            if ch == '$' {
                let should_exit = {
                    let tag = dollar_tag.as_deref().unwrap();
                    let close = format!("${}$", tag);
                    current.ends_with(&close)
                };
                if should_exit {
                    dollar_tag = None;
                }
            }
            prev = ch;
            continue;
        }

        // -- Line comment ----------------------------------------------------
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            current.push(ch);
            prev = ch;
            continue;
        }

        // -- Block comment ---------------------------------------------------
        if in_block_comment {
            if prev == '*' && ch == '/' {
                in_block_comment = false;
            }
            current.push(ch);
            prev = ch;
            continue;
        }

        // -- Single-quoted string --------------------------------------------
        if in_single_quote {
            current.push(ch);
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    current.push('\'');
                    chars.next();
                } else {
                    in_single_quote = false;
                }
            }
            prev = ch;
            continue;
        }

        match ch {
            '-' if chars.peek() == Some(&'-') => {
                in_line_comment = true;
                current.push(ch);
            }
            '/' if chars.peek() == Some(&'*') => {
                in_block_comment = true;
                current.push(ch);
            }
            '\'' => {
                in_single_quote = true;
                current.push(ch);
            }
            '$' => {
                // Attempt to parse a dollar-quote opening: $[tag]$
                // Consume valid tag characters [A-Za-z0-9_], then look for '$'.
                let mut tag = String::new();
                let mut found_close = false;
                while let Some(&next) = chars.peek() {
                    if next == '$' {
                        chars.next(); // consume the closing '$' of the tag
                        found_close = true;
                        break;
                    } else if next.is_ascii_alphanumeric() || next == '_' {
                        tag.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if found_close {
                    // Valid dollar-quote opening: push $tag$ and enter block mode.
                    current.push('$');
                    current.push_str(&tag);
                    current.push('$');
                    dollar_tag = Some(tag);
                } else {
                    // Not a dollar-quote; push the '$' and any collected chars.
                    current.push('$');
                    current.push_str(&tag);
                }
            }
            ';' => {
                statements.push(current.clone());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
        prev = ch;
    }

    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        statements.push(remaining);
    }

    statements
}

/// Detect whether a SQL string is incomplete (needs more input in the REPL).
///
/// Returns `true` if the statement has unclosed parentheses, unclosed
/// string literals, unclosed dollar-quoted blocks, or no terminating semicolon.
pub fn is_incomplete(sql: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    // Some(tag) when inside a dollar-quoted block.
    let mut dollar_tag: Option<String> = None;
    // Rolling buffer used only inside dollar-quoted blocks to detect the closing tag.
    let mut dollar_body = String::new();
    let mut chars = sql.chars().peekable();
    let mut has_semicolon = false;
    let mut prev = '\0';

    while let Some(ch) = chars.next() {
        // -- Dollar-quoted block ---------------------------------------------
        if dollar_tag.is_some() {
            dollar_body.push(ch);
            if ch == '$' {
                let should_exit = {
                    let tag = dollar_tag.as_deref().unwrap();
                    let close = format!("${}$", tag);
                    dollar_body.ends_with(&close)
                };
                if should_exit {
                    dollar_tag = None;
                    dollar_body.clear();
                }
            }
            prev = ch;
            continue;
        }

        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            prev = ch;
            continue;
        }
        if in_block_comment {
            if prev == '*' && ch == '/' {
                in_block_comment = false;
            }
            prev = ch;
            continue;
        }
        if in_single_quote {
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_single_quote = false;
                }
            }
            prev = ch;
            continue;
        }
        if in_double_quote {
            if ch == '"' {
                in_double_quote = false;
            }
            prev = ch;
            continue;
        }

        match ch {
            '-' if chars.peek() == Some(&'-') => in_line_comment = true,
            '/' if chars.peek() == Some(&'*') => in_block_comment = true,
            '\'' => in_single_quote = true,
            '"' => in_double_quote = true,
            '(' => depth += 1,
            ')' => depth -= 1,
            ';' => has_semicolon = true,
            '$' => {
                // Try to parse a dollar-quote opening.
                let mut tag = String::new();
                let mut found_close = false;
                while let Some(&next) = chars.peek() {
                    if next == '$' {
                        chars.next();
                        found_close = true;
                        break;
                    } else if next.is_ascii_alphanumeric() || next == '_' {
                        tag.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if found_close {
                    dollar_tag = Some(tag);
                    dollar_body.clear();
                }
                // If not found_close, '$' is a plain dollar (variable reference etc.)
            }
            _ => {}
        }
        prev = ch;
    }

    in_single_quote
        || in_double_quote
        || in_block_comment
        || dollar_tag.is_some()
        || depth > 0
        || !has_semicolon
}

/// Format a tokio-postgres error as a human-readable string, including a
/// caret indicator line when the server reports an error position.
pub(crate) fn format_db_error(e: &tokio_postgres::Error, sql: &str) -> String {
    let Some(db) = e.as_db_error() else {
        return e.to_string();
    };
    let base = format!("{}: {}", db.severity(), db.message());

    if let Some(tokio_postgres::error::ErrorPosition::Original(byte_pos)) = db.position() {
        let pos = (*byte_pos as usize).saturating_sub(1);
        let mut line_no = 1usize;
        let mut line_start = 0usize;
        for (i, ch) in sql.char_indices() {
            if i >= pos {
                break;
            }
            if ch == '\n' {
                line_no += 1;
                line_start = i + 1;
            }
        }
        let line_text = sql[line_start..].lines().next().unwrap_or("").trim_end();
        let col = pos.saturating_sub(line_start);
        let col = col.min(line_text.len());
        let prefix = format!("LINE {line_no}: ");
        let caret = " ".repeat(prefix.len() + col);
        return format!("{base}\n{prefix}{line_text}\n{caret}^");
    }

    base
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- split_statements ----------------------------------------------------

    #[test]
    fn split_simple_statements() {
        let stmts = split_statements("SELECT 1; SELECT 2;");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].trim(), "SELECT 1");
        assert_eq!(stmts[1].trim(), "SELECT 2");
    }

    #[test]
    fn split_respects_string_literals() {
        let stmts = split_statements("SELECT 'hello; world'; SELECT 2;");
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("hello; world"));
    }

    #[test]
    fn split_trailing_no_semicolon() {
        let stmts = split_statements("SELECT 1; SELECT 2");
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn split_dollar_quoted_function() {
        let sql = "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql;";
        let stmts = split_statements(sql);
        assert_eq!(
            stmts.len(),
            1,
            "semicolons inside $$ must not split the statement"
        );
        assert!(stmts[0].contains("BEGIN RETURN 1; END;"));
    }

    #[test]
    fn split_dollar_quoted_named_tag() {
        let sql =
            "CREATE FUNCTION f() RETURNS int AS $body$ BEGIN RETURN 1; END; $body$ LANGUAGE plpgsql;";
        let stmts = split_statements(sql);
        assert_eq!(
            stmts.len(),
            1,
            "named-tag dollar-quoted function should be one statement"
        );
    }

    #[test]
    fn split_dollar_quoted_then_next_statement() {
        let sql = "CREATE FUNCTION f() AS $$ RETURN 1; $$ LANGUAGE sql; SELECT 2;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("CREATE FUNCTION"));
        assert!(stmts[1].trim() == "SELECT 2");
    }

    #[test]
    fn split_dollar_sign_outside_quote() {
        // A bare $ (e.g. parameter reference $1 in psql) must not corrupt state.
        let stmts = split_statements("SELECT $1; SELECT 2;");
        assert_eq!(stmts.len(), 2);
    }

    // -- is_incomplete -------------------------------------------------------

    #[test]
    fn is_incomplete_no_semicolon() {
        assert!(is_incomplete("SELECT 1"));
    }

    #[test]
    fn is_incomplete_with_semicolon() {
        assert!(!is_incomplete("SELECT 1;"));
    }

    #[test]
    fn is_incomplete_open_paren() {
        assert!(is_incomplete("SELECT (1 + 2;"));
    }

    #[test]
    fn is_incomplete_open_string() {
        assert!(is_incomplete("SELECT 'unclosed"));
    }

    #[test]
    fn is_incomplete_multiline_complete() {
        let sql = "SELECT *\nFROM foo\nWHERE id = 1;";
        assert!(!is_incomplete(sql));
    }

    #[test]
    fn is_incomplete_open_dollar_quote() {
        assert!(is_incomplete(
            "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1;"
        ));
    }

    #[test]
    fn is_incomplete_closed_dollar_quote() {
        assert!(!is_incomplete(
            "CREATE FUNCTION f() RETURNS int AS $$ BEGIN RETURN 1; END; $$ LANGUAGE plpgsql;"
        ));
    }

    #[test]
    fn is_incomplete_named_dollar_quote_open() {
        assert!(is_incomplete("$body$ some content; no close yet"));
    }

    #[test]
    fn is_incomplete_named_dollar_quote_closed() {
        assert!(!is_incomplete("SELECT $body$ hello; world $body$::text;"));
    }
}
