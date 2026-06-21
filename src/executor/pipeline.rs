/// Script pipeline: execute a `.sql` file as a series of statements.
use std::path::Path;

use tokio_postgres::Client;
use tracing::info;

use crate::error::{PgCliError, Result};
use crate::executor::query::QueryExecutor;
use crate::protocol::messages::QueryResult;

/// Executes a SQL script file, optionally within a single transaction.
pub struct ScriptPipeline;

impl ScriptPipeline {
    /// Run all statements in `path` against `client`.
    ///
    /// If `single_transaction` is `true`, all statements are wrapped in a
    /// `BEGIN` / `COMMIT` block. Any error triggers `ROLLBACK`.
    ///
    /// Progress is logged at `info` level: statement index and content preview.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if the file cannot be read, or `PgCliError::Query`
    /// if any statement fails (after rolling back if in single-transaction mode).
    pub async fn run_file(
        client: &Client,
        path: &Path,
        single_transaction: bool,
    ) -> Result<Vec<QueryResult>> {
        let sql = std::fs::read_to_string(path).map_err(PgCliError::Io)?;

        if single_transaction {
            Self::run_in_transaction(client, &sql).await
        } else {
            Self::run_statements(client, &sql).await
        }
    }

    /// Execute a SQL string (possibly multi-statement) outside any explicit transaction.
    async fn run_statements(client: &Client, sql: &str) -> Result<Vec<QueryResult>> {
        let statements = split_script(sql);
        let mut results = Vec::with_capacity(statements.len());

        for (i, stmt) in statements.iter().enumerate() {
            let trimmed = stmt.trim();
            if trimmed.is_empty() {
                continue;
            }
            info!("Statement {}: {}", i + 1, preview(trimmed, 60));
            let result = QueryExecutor::execute(client, trimmed).await?;
            results.push(result);
        }

        Ok(results)
    }

    /// Wrap all statements in a `BEGIN` / `COMMIT`, rolling back on any error.
    async fn run_in_transaction(client: &Client, sql: &str) -> Result<Vec<QueryResult>> {
        client
            .execute("BEGIN", &[])
            .await
            .map_err(|e| PgCliError::Query(e.to_string()))?;

        match Self::run_statements(client, sql).await {
            Ok(results) => {
                client
                    .execute("COMMIT", &[])
                    .await
                    .map_err(|e| PgCliError::Query(e.to_string()))?;
                Ok(results)
            }
            Err(e) => {
                let _ = client.execute("ROLLBACK", &[]).await;
                Err(e)
            }
        }
    }
}

/// Split a SQL script into individual statements, respecting string literals
/// and comments (same logic as `query::split_statements` but for full files).
fn split_script(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut prev = '\0';
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_line_comment {
            current.push(ch);
            if ch == '\n' {
                in_line_comment = false;
            }
            prev = ch;
            continue;
        }
        if in_block_comment {
            current.push(ch);
            if prev == '*' && ch == '/' {
                in_block_comment = false;
            }
            prev = ch;
            continue;
        }
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
            ';' => {
                statements.push(std::mem::take(&mut current));
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

/// Return the first `max_chars` characters of `s` with `…` appended if truncated.
fn preview(s: &str, max_chars: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max_chars {
        s
    } else {
        format!("{}...", &s[..max_chars])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_script_basic() {
        let parts = split_script("SELECT 1;\nSELECT 2;\n");
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn split_script_skips_comment_semicolons() {
        // The semicolon inside the comment must not trigger a split.
        // The comment text is preserved in the buffer along with SELECT 1.
        let parts = split_script("-- this; is a comment\nSELECT 1;");
        assert_eq!(parts.len(), 1, "comment semicolon must not split");
        assert!(parts[0].contains("SELECT 1"));
    }

    #[test]
    fn split_script_preserves_string_semicolons() {
        let parts = split_script("SELECT 'a; b'; SELECT 2;");
        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("a; b"));
    }

    #[test]
    fn preview_truncates() {
        let p = preview("SELECT * FROM a_very_long_table_name WHERE id = 1", 20);
        assert!(p.ends_with("..."));
    }

    #[test]
    fn preview_no_truncate() {
        let p = preview("SELECT 1", 20);
        assert_eq!(p, "SELECT 1");
    }
}
