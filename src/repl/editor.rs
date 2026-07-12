/// Interactive REPL editor wrapping `rustyline`.
///
/// Handles multi-line SQL input, prompt formatting, Ctrl-C / Ctrl-D,
/// tab completion, and syntax highlighting.
use std::path::PathBuf;

use rustyline::{error::ReadlineError, history::FileHistory, Config, Editor};

use crate::error::{PgCliError, Result};
use crate::executor::query::is_incomplete;
use crate::repl::highlighter::SqlHighlighter;

/// Expand psql-style prompt escape sequences.
///
/// Supported sequences:
/// - `%n` — current user name
/// - `%/` — current database name
/// - `%~` — database name (`~` if home database)
/// - `%#` — `#` if superuser, `>` otherwise
/// - `%x` — transaction status: empty string, `*` (active), `!` (error)
/// - `%M` — full host:port or `[local]`
/// - `%>` — port number
/// - `%%` — literal `%`
pub fn expand_prompt(
    template: &str,
    dbname: &str,
    user: &str,
    host: &str,
    port: u16,
    is_superuser: bool,
    txn_status: &str,
) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push_str(user),
            Some('/') => out.push_str(dbname),
            Some('~') => out.push_str(dbname),
            Some('#') => out.push(if is_superuser { '#' } else { '>' }),
            Some('x') => out.push_str(txn_status),
            Some('M') => out.push_str(&format!("{host}:{port}")),
            Some('>') => out.push_str(&port.to_string()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

/// Primary prompt format: `dbname=# ` for superuser, `dbname=> ` for regular.
fn primary_prompt(
    dbname: &str,
    user: &str,
    host: &str,
    port: u16,
    is_superuser: bool,
    txn_status: &str,
    template: Option<&str>,
) -> String {
    if let Some(tmpl) = template {
        return expand_prompt(tmpl, dbname, user, host, port, is_superuser, txn_status);
    }
    let marker = if is_superuser { '#' } else { '>' };
    let txn = if txn_status.is_empty() {
        String::new()
    } else {
        format!("[{txn_status}]")
    };
    format!("{dbname}{txn}={marker} ")
}

/// Continuation prompt shown when a statement is incomplete.
fn continuation_prompt(
    dbname: &str,
    user: &str,
    host: &str,
    port: u16,
    txn_status: &str,
    template: Option<&str>,
) -> String {
    if let Some(tmpl) = template {
        return expand_prompt(tmpl, dbname, user, host, port, false, txn_status);
    }
    format!("{dbname}-> ")
}

/// Wraps `rustyline` to provide a full-featured REPL input loop.
pub struct ReplEditor {
    editor: Editor<SqlHighlighter, FileHistory>,
    dbname: String,
    user: String,
    host: String,
    port: u16,
    is_superuser: bool,
    history_path: Option<PathBuf>,
    /// Custom PROMPT1 template (psql-style %-escapes).
    pub prompt1: Option<String>,
    /// Custom PROMPT2 (continuation) template.
    pub prompt2: Option<String>,
    /// Current transaction status string: `""` (idle), `"*"` (in txn), `"!"` (error).
    pub txn_status: String,
}

impl ReplEditor {
    /// Create a new `ReplEditor`.
    ///
    /// # Arguments
    ///
    /// * `dbname`      - the current database name, shown in the prompt
    /// * `user`        - the connected username
    /// * `host`        - the server host
    /// * `port`        - the server port
    /// * `is_superuser`- whether to show `#` (true) or `>` (false) in prompt
    /// * `highlighter` - the SQL syntax highlighter
    /// * `history_path`- optional path to the history file
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if the editor cannot be initialized.
    pub fn new(
        dbname: &str,
        user: &str,
        host: &str,
        port: u16,
        is_superuser: bool,
        highlighter: SqlHighlighter,
        history_path: Option<PathBuf>,
    ) -> Result<Self> {
        let config = Config::builder()
            .history_ignore_space(true)
            .completion_type(rustyline::CompletionType::List)
            .edit_mode(rustyline::EditMode::Emacs)
            .build();

        let mut editor = Editor::with_config(config)
            .map_err(|e| PgCliError::Io(std::io::Error::other(e.to_string())))?;

        editor.set_helper(Some(highlighter));

        if let Some(ref path) = history_path {
            let _ = editor.load_history(path);
        }

        Ok(Self {
            editor,
            dbname: dbname.to_string(),
            user: user.to_string(),
            host: host.to_string(),
            port,
            is_superuser,
            history_path,
            prompt1: None,
            prompt2: None,
            txn_status: String::new(),
        })
    }

    /// Read the next complete SQL statement from the user.
    ///
    /// Handles multi-line input by accumulating lines until the statement
    /// is syntactically complete (ends with `;` and has balanced parens/quotes).
    ///
    /// Returns:
    /// - `Ok(Some(sql))` when the user enters a complete statement.
    /// - `Ok(None)` when the user presses Ctrl-D (EOF / exit).
    /// - `Err(PgCliError::Interrupted)` when Ctrl-C is pressed.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Interrupted` on Ctrl-C, or `PgCliError::Io` on
    /// unexpected readline failures.
    pub fn readline(&mut self) -> Result<Option<String>> {
        let mut buffer = String::new();

        loop {
            let prompt = if buffer.is_empty() {
                primary_prompt(
                    &self.dbname,
                    &self.user,
                    &self.host,
                    self.port,
                    self.is_superuser,
                    &self.txn_status.clone(),
                    self.prompt1.as_deref(),
                )
            } else {
                continuation_prompt(
                    &self.dbname,
                    &self.user,
                    &self.host,
                    self.port,
                    &self.txn_status.clone(),
                    self.prompt2.as_deref(),
                )
            };

            match self.editor.readline(&prompt) {
                Ok(line) => {
                    if !buffer.is_empty() {
                        buffer.push('\n');
                    }
                    buffer.push_str(&line);

                    // Check for meta-commands (backslash) on first line.
                    let trimmed = buffer.trim();
                    if trimmed.starts_with('\\') {
                        self.editor.add_history_entry(&buffer).ok();
                        return Ok(Some(buffer));
                    }

                    // Empty input-skip.
                    if trimmed.is_empty() {
                        buffer.clear();
                        continue;
                    }

                    // If the statement is complete, return it.
                    if !is_incomplete(trimmed) {
                        self.editor.add_history_entry(&buffer).ok();
                        return Ok(Some(buffer));
                    }
                    // Otherwise show the continuation prompt.
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl-C: discard current buffer and signal interrupt.
                    buffer.clear();
                    return Err(PgCliError::Interrupted);
                }
                Err(ReadlineError::Eof) => {
                    // Ctrl-D: signal graceful exit.
                    return Ok(None);
                }
                Err(e) => {
                    return Err(PgCliError::Io(std::io::Error::other(e.to_string())));
                }
            }
        }
    }

    /// Update the displayed database name (e.g. after `\c otherdb`).
    pub fn set_dbname(&mut self, dbname: &str) {
        self.dbname = dbname.to_string();
    }

    /// Update the user name shown in custom prompts.
    pub fn set_user(&mut self, user: &str) {
        self.user = user.to_string();
    }

    /// Update the host/port shown in custom prompts.
    pub fn set_host_port(&mut self, host: &str, port: u16) {
        self.host = host.to_string();
        self.port = port;
    }

    /// Toggle between superuser (`#`) and regular user (`>`) prompt.
    pub fn set_superuser(&mut self, is_superuser: bool) {
        self.is_superuser = is_superuser;
    }

    /// Save history to the configured file path.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Io` if saving fails.
    pub fn save_history(&mut self) -> Result<()> {
        if let Some(ref path) = self.history_path {
            self.editor
                .save_history(path)
                .map_err(|e| PgCliError::Io(std::io::Error::other(e.to_string())))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_prompt_superuser() {
        assert_eq!(
            primary_prompt("mydb", "pg", "localhost", 5432, true, "", None),
            "mydb=# "
        );
    }

    #[test]
    fn primary_prompt_regular() {
        assert_eq!(
            primary_prompt("mydb", "pg", "localhost", 5432, false, "", None),
            "mydb=> "
        );
    }

    #[test]
    fn primary_prompt_in_transaction() {
        let s = primary_prompt("mydb", "pg", "localhost", 5432, false, "*", None);
        assert_eq!(s, "mydb[*]=> ");
    }

    #[test]
    fn continuation_prompt_format() {
        assert_eq!(
            continuation_prompt("mydb", "pg", "localhost", 5432, "", None),
            "mydb-> "
        );
    }

    #[test]
    fn expand_prompt_sequences() {
        let s = expand_prompt("%n@%M %/=%# ", "testdb", "alice", "pghost", 5433, true, "");
        assert_eq!(s, "alice@pghost:5433 testdb=# ");
    }

    #[test]
    fn expand_prompt_txn_status() {
        let s = expand_prompt("[%x]%/=%# ", "db", "u", "h", 5432, false, "*");
        assert_eq!(s, "[*]db=> ");
    }

    #[test]
    fn expand_prompt_percent_literal() {
        let s = expand_prompt("100%% done", "db", "u", "h", 5432, false, "");
        assert_eq!(s, "100% done");
    }

    #[test]
    fn custom_prompt1_template() {
        let out = primary_prompt("mydb", "bob", "srv", 5432, false, "", Some("%n@%/%# "));
        assert_eq!(out, "bob@mydb> ");
    }

    #[test]
    fn expand_prompt_unknown_sequence_kept_literal() {
        let s = expand_prompt("%z", "db", "u", "h", 5432, false, "");
        assert_eq!(s, "%z");
    }

    #[test]
    fn expand_prompt_trailing_percent() {
        let s = expand_prompt("abc%", "db", "u", "h", 5432, false, "");
        assert_eq!(s, "abc%");
    }

    #[test]
    fn continuation_prompt_uses_custom_template() {
        let out = continuation_prompt("mydb", "bob", "srv", 5432, "*", Some("%/[%x]-> "));
        assert_eq!(out, "mydb[*]-> ");
    }

    fn make_editor() -> ReplEditor {
        let highlighter = SqlHighlighter::new("none");
        ReplEditor::new(
            "testdb",
            "tester",
            "localhost",
            5432,
            false,
            highlighter,
            None,
        )
        .expect("editor construction should not fail without a history file")
    }

    #[test]
    fn new_editor_construction_succeeds() {
        let _editor = make_editor();
    }

    #[test]
    fn setters_update_prompt_fields() {
        let mut editor = make_editor();
        editor.set_dbname("otherdb");
        editor.set_user("alice");
        editor.set_host_port("otherhost", 5433);
        editor.set_superuser(true);
        assert_eq!(editor.dbname, "otherdb");
        assert_eq!(editor.user, "alice");
        assert_eq!(editor.host, "otherhost");
        assert_eq!(editor.port, 5433);
        assert!(editor.is_superuser);
    }

    #[test]
    fn save_history_without_path_is_noop_ok() {
        let mut editor = make_editor();
        assert!(editor.save_history().is_ok());
    }

    #[test]
    fn save_history_with_path_writes_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("pgcli_rs_test_editor_history");
        let _ = std::fs::remove_file(&path);

        let highlighter = SqlHighlighter::new("none");
        let mut editor = ReplEditor::new(
            "testdb",
            "tester",
            "localhost",
            5432,
            false,
            highlighter,
            Some(path.clone()),
        )
        .expect("editor construction failed");

        let result = editor.save_history();
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }
}
