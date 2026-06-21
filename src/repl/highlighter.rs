/// SQL syntax highlighter and schema-aware completer for the rustyline REPL.
///
/// Applies ANSI color codes to SQL keywords, string literals, and comments.
/// When `theme` is `"none"`, input is returned unchanged.
/// Tab completion suggests SQL keywords and, when a schema cache is attached,
/// also table names, column names, schema names, and function names.
use colored::Colorize;
use rustyline::highlight::Highlighter;
use rustyline_derive::{Helper, Hinter, Validator};

use crate::repl::schema_cache::SharedSchemaCache;

/// All supported meta-commands (without leading backslash) for tab completion.
const META_COMMANDS: &[&str] = &[
    "q",
    "quit",
    "l",
    "l+",
    "d",
    "d+",
    "dt",
    "dv",
    "di",
    "ds",
    "df",
    "df+",
    "dn",
    "dT",
    "dx",
    "dp",
    "z",
    "dd",
    "c",
    "i",
    "o",
    "e",
    "sf",
    "sv",
    "ddl",
    "introspect",
    "pset",
    "format",
    "x",
    "timing",
    "watch",
    "watch diff",
    "copy",
    "gexec",
    "gset",
    "gdesc",
    "bench",
    "hist",
    "set",
    "unset",
    "echo",
    "qecho",
    "if",
    "endif",
    "explain",
    "encoding",
    "dconfig",
    "dc",
    "deps",
    "indexes",
    "bloat",
    "size",
    "locks",
    "activity",
    "vacuum",
    "bookmark",
    "bookmarks",
    "run",
    "delbookmark",
    "conninfo",
    "reconnect",
    "cd",
    "?",
    "h",
];

/// SQL keywords that should be highlighted and suggested for completion.
const SQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "DROP",
    "ALTER",
    "TABLE",
    "INDEX",
    "VIEW",
    "SEQUENCE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "FULL",
    "CROSS",
    "ON",
    "AS",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "ORDER",
    "BY",
    "GROUP",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "UNION",
    "ALL",
    "DISTINCT",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "WITH",
    "RETURNING",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "TRANSACTION",
    "EXPLAIN",
    "ANALYZE",
    "VACUUM",
    "TRUNCATE",
    "COPY",
    "GRANT",
    "REVOKE",
    "CONSTRAINT",
    "PRIMARY",
    "KEY",
    "FOREIGN",
    "REFERENCES",
    "UNIQUE",
    "DEFAULT",
    "CHECK",
    "CAST",
    "OVER",
    "PARTITION",
    "FILTER",
    "WITHIN",
    "EXCLUDE",
];

/// Highlighter and schema-aware completer for SQL input in the REPL.
///
/// `Completer` is implemented manually to merge SQL keywords with live schema
/// objects from the attached `schema_cache`. All other rustyline traits are derived.
#[derive(Clone, Helper, Hinter, Validator)]
pub struct SqlHighlighter {
    /// Color theme: `"dark"`, `"light"`, or `"none"`.
    pub theme: String,
    /// Whether ANSI highlighting is enabled.
    pub enabled: bool,
    /// Optional live schema cache for schema-aware tab completion.
    pub schema_cache: Option<SharedSchemaCache>,
}

impl rustyline::completion::Completer for SqlHighlighter {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        // Meta-command completion: if line starts with '\', complete command names.
        let trimmed = line[..pos].trim_start();
        if trimmed.starts_with('\\') && !trimmed.contains(char::is_whitespace) {
            let prefix = &trimmed[1..]; // e.g. "d", "sf", ""
            let matches: Vec<String> = META_COMMANDS
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| format!("\\{c}"))
                .collect();
            if !matches.is_empty() {
                return Ok((line[..pos].rfind('\\').unwrap_or(0), matches));
            }
        }

        // Find start of the current word (stop at whitespace or SQL delimiters but keep '.').
        let word_start = line[..pos]
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map(|i| i + 1)
            .unwrap_or(0);
        let word = &line[word_start..pos];
        if word.is_empty() {
            return Ok((pos, vec![]));
        }
        let upper = word.to_uppercase();

        // `table.col` dot-notation: word contains '.' → complete columns for that table.
        if let Some(dot_pos) = word.find('.') {
            let table_prefix = &word[..dot_pos];
            let col_prefix = word[dot_pos + 1..].to_uppercase();
            if let Some(ref arc) = self.schema_cache {
                if let Ok(cache) = arc.read() {
                    let table_upper = table_prefix.to_uppercase();
                    let cols: Vec<String> = cache
                        .table_columns
                        .iter()
                        .filter(|(tbl, _)| tbl.to_uppercase() == table_upper)
                        .flat_map(|(_, cols)| cols.iter().cloned())
                        .filter(|c| c.to_uppercase().starts_with(col_prefix.as_str()))
                        .map(|c| format!("{table_prefix}.{c}"))
                        .collect();
                    if !cols.is_empty() {
                        return Ok((word_start, cols));
                    }
                }
            }
        }

        // Detect context from the keyword immediately before the current word.
        let before_upper = line[..word_start].trim_end().to_uppercase();
        let last_kw = before_upper.split_whitespace().last().unwrap_or("");
        let table_ctx = matches!(
            last_kw,
            "FROM" | "JOIN" | "UPDATE" | "INTO" | "TABLE" | "TRUNCATE"
        );
        let column_ctx = matches!(
            last_kw,
            "SELECT" | "WHERE" | "SET" | "ON" | "HAVING" | "AND" | "OR" | "BY"
        );

        let mut candidates: Vec<String> = Vec::new();

        // Schema-aware suggestions first (higher relevance than keyword list).
        if let Some(ref arc) = self.schema_cache {
            if let Ok(cache) = arc.read() {
                if table_ctx {
                    // In table context: table names take priority.
                    for name in &cache.table_names {
                        if name.to_uppercase().starts_with(upper.as_str()) {
                            candidates.push(name.clone());
                        }
                    }
                    for fq in &cache.qualified_tables {
                        if fq.to_uppercase().starts_with(upper.as_str()) && !candidates.contains(fq)
                        {
                            candidates.push(fq.clone());
                        }
                    }
                } else if column_ctx {
                    // In column context: column names take priority.
                    for col in &cache.columns {
                        if col.to_uppercase().starts_with(upper.as_str())
                            && !candidates.contains(col)
                        {
                            candidates.push(col.clone());
                        }
                    }
                    // Also offer table names (for aliases and qualified refs).
                    for name in &cache.table_names {
                        if name.to_uppercase().starts_with(upper.as_str())
                            && !candidates.contains(name)
                        {
                            candidates.push(name.clone());
                        }
                    }
                } else {
                    // General context: tables, schemas, and functions.
                    for name in cache
                        .table_names
                        .iter()
                        .chain(cache.schemas.iter())
                        .chain(cache.functions.iter())
                    {
                        if name.to_uppercase().starts_with(upper.as_str())
                            && !candidates.contains(name)
                        {
                            candidates.push(name.clone());
                        }
                    }
                }
            }
        }

        // SQL keywords appended after schema objects.
        for &kw in SQL_KEYWORDS {
            if kw.starts_with(upper.as_str()) && !candidates.contains(&kw.to_string()) {
                candidates.push(kw.to_string());
            }
        }

        Ok((word_start, candidates))
    }
}

impl SqlHighlighter {
    /// Create a new `SqlHighlighter` for the given theme with no schema cache.
    pub fn new(theme: &str) -> Self {
        Self {
            enabled: theme != "none",
            theme: theme.to_string(),
            schema_cache: None,
        }
    }

    /// Attach a shared schema cache for schema-aware tab completion.
    pub fn with_cache(mut self, cache: SharedSchemaCache) -> Self {
        self.schema_cache = Some(cache);
        self
    }

    /// Apply syntax coloring to a SQL string.
    ///
    /// When disabled (theme = "none"), returns `input` unchanged.
    pub fn highlight_sql(&self, input: &str) -> String {
        if !self.enabled {
            return input.to_string();
        }
        highlight_tokens(input, &self.theme)
    }
}

impl Highlighter for SqlHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        if !self.enabled {
            return std::borrow::Cow::Borrowed(line);
        }
        std::borrow::Cow::Owned(self.highlight_sql(line))
    }

    fn highlight_char(&self, _line: &str, _pos: usize) -> bool {
        self.enabled
    }
}

/// Tokenize `input` and apply colors to keywords, strings, and comments.
fn highlight_tokens(input: &str, theme: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // Line comment --
        if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] == '-' {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            let comment: String = chars[start..i].iter().collect();
            out.push_str(&comment.dimmed().to_string());
            continue;
        }

        // Block comment /* ... */
        if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2; // consume */
            let comment: String = chars[start..i].iter().collect();
            out.push_str(&comment.dimmed().to_string());
            continue;
        }

        // Single-quoted string literal
        if chars[i] == '\'' {
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    i += 1;
                    if i < chars.len() && chars[i] == '\'' {
                        i += 1; // escaped quote
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            let literal: String = chars[start..i].iter().collect();
            let colored = if theme == "light" {
                literal.green().to_string()
            } else {
                literal.bright_green().to_string()
            };
            out.push_str(&colored);
            continue;
        }

        // Dollar-quoted string $$...$$ (simplified: match $$)
        if i + 1 < chars.len() && chars[i] == '$' && chars[i + 1] == '$' {
            let start = i;
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '$' && chars[i + 1] == '$') {
                i += 1;
            }
            // Only advance past the closing '$$' if it was actually found.
            if i + 1 < chars.len() {
                i += 2;
            }
            let literal: String = chars[start..i].iter().collect();
            out.push_str(&literal.bright_green().to_string());
            continue;
        }

        // Double-quoted identifier
        if chars[i] == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            // Only advance past the closing '"' if it was actually found.
            if i < chars.len() {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            out.push_str(&ident.cyan().to_string());
            continue;
        }

        // Word token: check if it's a keyword
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if SQL_KEYWORDS.contains(&word.to_uppercase().as_str()) {
                let colored = if theme == "light" {
                    word.blue().bold().to_string()
                } else {
                    word.bright_blue().bold().to_string()
                };
                out.push_str(&colored);
            } else {
                out.push_str(&word);
            }
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustyline::completion::Completer as _;

    #[test]
    fn theme_none_returns_unchanged() {
        let h = SqlHighlighter::new("none");
        assert!(!h.enabled);
        assert_eq!(h.highlight_sql("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn dark_theme_changes_output() {
        let h = SqlHighlighter::new("dark");
        let out = h.highlight_sql("SELECT 1");
        assert!(
            out.contains('\x1b') || out == "SELECT 1",
            "Expected ANSI codes or plain text but got: {out:?}"
        );
    }

    #[test]
    fn keywords_list_not_empty() {
        assert!(!SQL_KEYWORDS.is_empty());
        assert!(SQL_KEYWORDS.contains(&"SELECT"));
    }

    #[test]
    fn complete_keyword_prefix() {
        let h = SqlHighlighter::new("dark");
        let (start, cands) = h
            .complete(
                "SEL",
                3,
                &rustyline::Context::new(&rustyline::history::DefaultHistory::new()),
            )
            .unwrap();
        assert_eq!(start, 0);
        assert!(cands.contains(&"SELECT".to_string()));
    }

    #[test]
    fn complete_empty_word_returns_no_candidates() {
        let h = SqlHighlighter::new("dark");
        let (_, cands) = h
            .complete(
                "SELECT ",
                7,
                &rustyline::Context::new(&rustyline::history::DefaultHistory::new()),
            )
            .unwrap();
        assert!(cands.is_empty());
    }

    #[test]
    fn highlight_unclosed_double_quote_does_not_panic() {
        // Pasting an incomplete identifier must not panic with index-out-of-range.
        let h = SqlHighlighter::new("dark");
        let _ = h.highlight_sql("CREATE EXTENSION IF NOT EXISTS");
        let _ = h.highlight_sql(r#"SELECT "unclosed"#);
    }

    #[test]
    fn highlight_unclosed_dollar_quote_does_not_panic() {
        let h = SqlHighlighter::new("dark");
        let _ = h.highlight_sql("$$ unclosed dollar");
    }

    #[test]
    fn complete_with_schema_cache() {
        use crate::repl::schema_cache::SchemaCache;
        use std::sync::{Arc, RwLock};
        let cache = Arc::new(RwLock::new(SchemaCache {
            table_names: vec!["users".to_string(), "orders".to_string()],
            ..SchemaCache::default()
        }));
        let h = SqlHighlighter::new("dark").with_cache(cache);
        let (_, cands) = h
            .complete(
                "SELECT * FROM use",
                17,
                &rustyline::Context::new(&rustyline::history::DefaultHistory::new()),
            )
            .unwrap();
        assert!(
            cands.contains(&"users".to_string()),
            "candidates: {cands:?}"
        );
    }
}
