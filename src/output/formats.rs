/// Output format selection and `Formatter` trait.
use crate::error::Result;
use crate::protocol::messages::QueryResult;

/// All supported output formats.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum OutputFormat {
    /// Aligned table using box-drawing characters (default).
    #[default]
    Table,
    /// CSV with header row.
    Csv,
    /// JSON array of objects.
    Json,
    /// JSON Lines (one object per line).
    Jsonl,
    /// Tab-separated values.
    Tsv,
    /// HTML `<table>` element.
    Html,
    /// Unaligned text with configurable field/record separators.
    Unaligned,
    /// GitHub-Flavored Markdown pipe table.
    Markdown,
    /// LaTeX `tabular` environment.
    Latex,
    /// AsciiDoc table syntax.
    Asciidoc,
}

impl std::str::FromStr for OutputFormat {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, ()> {
        match s.to_lowercase().as_str() {
            "table" => Ok(Self::Table),
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            "jsonl" | "ndjson" => Ok(Self::Jsonl),
            "tsv" => Ok(Self::Tsv),
            "html" => Ok(Self::Html),
            "unaligned" => Ok(Self::Unaligned),
            "markdown" | "md" => Ok(Self::Markdown),
            "latex" | "tex" => Ok(Self::Latex),
            "asciidoc" | "adoc" => Ok(Self::Asciidoc),
            _ => Err(()),
        }
    }
}

impl OutputFormat {
    /// Return the canonical format name string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Csv => "csv",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
            Self::Tsv => "tsv",
            Self::Html => "html",
            Self::Unaligned => "unaligned",
            Self::Markdown => "markdown",
            Self::Latex => "latex",
            Self::Asciidoc => "asciidoc",
        }
    }
}

/// Border style for table output.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LineStyle {
    /// Standard ASCII characters (`+`, `-`, `|`).
    Ascii,
    /// Unicode box-drawing characters.
    #[default]
    Unicode,
    /// Old-style ASCII (psql `\pset linestyle old-ascii`).
    OldAscii,
}

/// Options controlling how a `QueryResult` is formatted.
#[derive(Debug, Clone)]
pub struct FormatOptions {
    /// Suppress column headers and row-count footer.
    pub tuples_only: bool,
    /// Show the `(N rows)` footer line (independent of `tuples_only`).
    pub footer: bool,
    /// Field separator used in unaligned and CSV modes.
    pub field_separator: String,
    /// Record (row) separator used in unaligned mode.
    pub record_separator: String,
    /// String displayed in place of NULL values.
    pub null_display: String,
    /// Border level: 0 = none, 1 = inner rules only, 2 = full outer border.
    pub border: u8,
    /// Expanded (vertical) display: each row becomes a column=value list.
    pub expanded: bool,
    /// Line/border drawing style.
    pub line_style: LineStyle,
    /// Maximum cell content width before truncation (0 = unlimited).
    pub max_column_width: usize,
    /// Show query timing footer.
    pub timing: bool,
    /// Color theme: `dark`, `light`, or `none`.
    pub theme: String,
    /// Optional title printed above the result table.
    pub title: Option<String>,
    /// Use locale-specific number formatting (thousands separators).
    pub numeric_locale: bool,
    /// Whether the pager is enabled at runtime (can be toggled with \pset pager).
    pub pager_enabled: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            tuples_only: false,
            footer: true,
            field_separator: "|".to_string(),
            record_separator: "\n".to_string(),
            null_display: String::new(),
            border: 1,
            expanded: false,
            line_style: LineStyle::Unicode,
            max_column_width: 0,
            timing: true,
            theme: "dark".to_string(),
            title: None,
            numeric_locale: false,
            pager_enabled: true,
        }
    }
}

/// Format a query duration as a human-readable string.
///
/// - Under 1 ms  → `"< 1 ms"`
/// - Under 1 s   → `"N ms"` (integer)
/// - Under 1 min → `"N.NNN s"`
/// - Otherwise   → `"Nm Ns.SSS s"`
pub fn format_duration(ms: u64) -> String {
    if ms == 0 {
        return "< 1 ms".to_string();
    }
    if ms < 1_000 {
        return format!("{ms} ms");
    }
    if ms < 60_000 {
        let secs = ms as f64 / 1_000.0;
        return format!("{secs:.3} s");
    }
    let mins = ms / 60_000;
    let secs = (ms % 60_000) as f64 / 1_000.0;
    format!("{mins}m {secs:.3} s")
}

/// Colorize an `EXPLAIN` / `EXPLAIN ANALYZE` plan text.
///
/// Highlights expensive nodes (Seq Scans, high costs, long actual times) with
/// colored output. Pass `is_terminal = false` to suppress ANSI codes.
pub fn colorize_explain_plan(rows: &[String], is_terminal: bool) -> String {
    use colored::Colorize;
    let mut out = String::new();
    for line in rows {
        if !is_terminal {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Extract actual time if present: "(actual time=N..M"
        let actual_ms: Option<f64> = {
            line.find("actual time=").and_then(|p| {
                let rest = &line[p + 12..];
                let end = rest.find("..").unwrap_or(rest.len());
                rest[..end].parse::<f64>().ok()
            })
        };
        // Extract estimated cost: "(cost=N..M"
        let est_cost: Option<f64> = {
            line.find("cost=").and_then(|p| {
                let rest = &line[p + 5..];
                let dot2 = rest.find("..").unwrap_or(0);
                let end = rest[dot2..].find(' ').unwrap_or(rest.len() - dot2) + dot2;
                rest[dot2 + 2..end].parse::<f64>().ok()
            })
        };
        let colored_line = if line.trim_start().starts_with("Seq Scan") {
            line.yellow().to_string()
        } else if actual_ms.map(|t| t > 100.0).unwrap_or(false) {
            line.red().bold().to_string()
        } else if actual_ms.map(|t| t > 10.0).unwrap_or(false) {
            line.yellow().to_string()
        } else if est_cost.map(|c| c > 10_000.0).unwrap_or(false) {
            line.magenta().to_string()
        } else if line.trim_start().starts_with("Execution Time")
            || line.trim_start().starts_with("Planning Time")
        {
            line.cyan().to_string()
        } else {
            line.clone()
        };
        out.push_str(&colored_line);
        out.push('\n');
    }
    out
}

/// Trait implemented by every output format renderer.
pub trait Formatter {
    /// Format `result` according to `opts` and return the complete output string.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if serialization fails.
    fn format(&self, result: &QueryResult, opts: &FormatOptions) -> Result<String>;
}

/// Dispatch a `QueryResult` through the correct `Formatter` for `format`.
///
/// # Errors
///
/// Returns `PgCliError::Export` if formatting fails.
pub fn format_result(
    result: &QueryResult,
    format: &OutputFormat,
    opts: &FormatOptions,
) -> Result<String> {
    use crate::export::{csv::CsvExporter, json::JsonExporter};
    use crate::output::table::TableFormatter;

    match format {
        OutputFormat::Table => TableFormatter.format(result, opts),
        OutputFormat::Csv => {
            let mut buf = Vec::new();
            CsvExporter::export(result, &mut buf)?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        }
        OutputFormat::Json => {
            let mut buf = Vec::new();
            JsonExporter::export_array(result, &mut buf)?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        }
        OutputFormat::Jsonl => {
            let mut buf = Vec::new();
            JsonExporter::export_lines(result, &mut buf)?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        }
        OutputFormat::Tsv => format_tsv(result, opts),
        OutputFormat::Html => format_html(result, opts),
        OutputFormat::Unaligned => format_unaligned(result, opts),
        OutputFormat::Markdown => format_markdown(result, opts),
        OutputFormat::Latex => format_latex(result, opts),
        OutputFormat::Asciidoc => format_asciidoc(result, opts),
    }
}

/// Format as tab-separated values.
fn format_tsv(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let mut out = String::new();
    if !opts.tuples_only {
        let header: Vec<_> = result.columns.iter().map(|c| c.name.as_str()).collect();
        out.push_str(&header.join("\t"));
        out.push('\n');
    }
    for row in &result.rows {
        let cells: Vec<String> = row
            .values
            .iter()
            .zip(&result.columns)
            .map(|(v, _)| {
                let s = v.to_string();
                if s.is_empty() {
                    opts.null_display.clone()
                } else {
                    s
                }
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    Ok(out)
}

/// Format as an HTML `<table>`.
fn format_html(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let mut out = String::from("<table>\n");
    if !opts.tuples_only {
        out.push_str("  <thead><tr>");
        for col in &result.columns {
            out.push_str(&format!("<th>{}</th>", html_escape(&col.name)));
        }
        out.push_str("</tr></thead>\n");
    }
    out.push_str("  <tbody>\n");
    for row in &result.rows {
        out.push_str("    <tr>");
        for (val, _col) in row.values.iter().zip(&result.columns) {
            let s = val.to_string();
            let display = if s.is_empty() {
                opts.null_display.clone()
            } else {
                s
            };
            out.push_str(&format!("<td>{}</td>", html_escape(&display)));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("  </tbody>\n</table>");
    Ok(out)
}

/// Format with configurable field and record separators.
fn format_unaligned(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let mut out = String::new();
    if !opts.tuples_only {
        let header: Vec<_> = result.columns.iter().map(|c| c.name.as_str()).collect();
        out.push_str(&header.join(&opts.field_separator));
        out.push_str(&opts.record_separator);
    }
    for row in &result.rows {
        let cells: Vec<String> = row
            .values
            .iter()
            .map(|v| {
                let s = v.to_string();
                if s.is_empty() {
                    opts.null_display.clone()
                } else {
                    s
                }
            })
            .collect();
        out.push_str(&cells.join(&opts.field_separator));
        out.push_str(&opts.record_separator);
    }
    Ok(out)
}

/// Format as a GitHub-Flavored Markdown pipe table.
fn format_markdown(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let null = &opts.null_display;
    let mut out = String::new();

    if !opts.tuples_only {
        // Header row.
        let headers: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        out.push('|');
        for h in &headers {
            out.push_str(&format!(" {h} |"));
        }
        out.push('\n');
        // Separator row.
        out.push('|');
        for _ in &headers {
            out.push_str(" --- |");
        }
        out.push('\n');
    }

    for row in &result.rows {
        out.push('|');
        for val in &row.values {
            let s = if matches!(val, crate::protocol::messages::CellValue::Null) {
                null.clone()
            } else {
                val.to_string().replace('|', "\\|").replace('\n', "<br>")
            };
            out.push_str(&format!(" {s} |"));
        }
        out.push('\n');
    }
    Ok(out)
}

/// Format as a LaTeX `tabular` environment.
fn format_latex(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let null = &opts.null_display;
    let ncols = result.columns.len();
    let col_spec: String = "l".repeat(ncols.max(1));
    let mut out = format!("\\begin{{tabular}}{{{col_spec}}}\n\\hline\n");

    if !opts.tuples_only {
        let headers: Vec<String> = result
            .columns
            .iter()
            .map(|c| latex_escape(&c.name))
            .collect();
        out.push_str(&headers.join(" & "));
        out.push_str(" \\\\\n\\hline\n");
    }

    for row in &result.rows {
        let cells: Vec<String> = row
            .values
            .iter()
            .map(|v| {
                if matches!(v, crate::protocol::messages::CellValue::Null) {
                    latex_escape(null)
                } else {
                    latex_escape(&v.to_string())
                }
            })
            .collect();
        out.push_str(&cells.join(" & "));
        out.push_str(" \\\\\n");
    }
    out.push_str("\\hline\n\\end{tabular}\n");
    Ok(out)
}

/// Escape LaTeX special characters.
fn latex_escape(s: &str) -> String {
    s.replace('\\', "\\textbackslash{}")
        .replace('&', "\\&")
        .replace('%', "\\%")
        .replace('$', "\\$")
        .replace('#', "\\#")
        .replace('_', "\\_")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('~', "\\textasciitilde{}")
        .replace('^', "\\textasciicircum{}")
}

/// Format as an AsciiDoc table.
fn format_asciidoc(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let null = &opts.null_display;
    let ncols = result.columns.len();
    let mut out = "|===\n".to_string();

    if !opts.tuples_only {
        for col in &result.columns {
            out.push_str(&format!("| {}", col.name));
        }
        out.push('\n');
    }
    out.push('\n');

    for row in &result.rows {
        for val in row.values.iter() {
            let s = if matches!(val, crate::protocol::messages::CellValue::Null) {
                null.clone()
            } else {
                val.to_string()
            };
            out.push_str(&format!("| {s}\n"));
        }
        out.push('\n');
    }
    let _ = ncols;
    out.push_str("|===\n");
    Ok(out)
}

/// Escape HTML special characters.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_format_round_trip() {
        assert_eq!("table".parse::<OutputFormat>(), Ok(OutputFormat::Table));
        assert_eq!("JSON".parse::<OutputFormat>(), Ok(OutputFormat::Json));
        assert_eq!("ndjson".parse::<OutputFormat>(), Ok(OutputFormat::Jsonl));
        assert_eq!("bogus".parse::<OutputFormat>(), Err(()));
    }

    #[test]
    fn html_escape_special_chars() {
        assert_eq!(html_escape("<b>"), "&lt;b&gt;");
        assert_eq!(html_escape("a & b"), "a &amp; b");
    }

    #[test]
    fn format_options_default_border() {
        let opts = FormatOptions::default();
        assert_eq!(opts.border, 1);
        assert!(opts.footer);
        assert!(opts.title.is_none());
    }

    #[test]
    fn format_duration_ranges() {
        assert_eq!(format_duration(0), "< 1 ms");
        assert_eq!(format_duration(1), "1 ms");
        assert_eq!(format_duration(999), "999 ms");
        assert_eq!(format_duration(1_000), "1.000 s");
        assert_eq!(format_duration(1_234), "1.234 s");
        assert_eq!(format_duration(59_999), "59.999 s");
        assert_eq!(format_duration(60_000), "1m 0.000 s");
        assert_eq!(format_duration(90_500), "1m 30.500 s");
    }

    #[test]
    fn output_format_new_variants_round_trip() {
        assert_eq!(
            "markdown".parse::<OutputFormat>(),
            Ok(OutputFormat::Markdown)
        );
        assert_eq!("md".parse::<OutputFormat>(), Ok(OutputFormat::Markdown));
        assert_eq!("latex".parse::<OutputFormat>(), Ok(OutputFormat::Latex));
        assert_eq!("tex".parse::<OutputFormat>(), Ok(OutputFormat::Latex));
        assert_eq!(
            "asciidoc".parse::<OutputFormat>(),
            Ok(OutputFormat::Asciidoc)
        );
        assert_eq!("adoc".parse::<OutputFormat>(), Ok(OutputFormat::Asciidoc));
        assert_eq!(OutputFormat::Markdown.as_str(), "markdown");
        assert_eq!(OutputFormat::Latex.as_str(), "latex");
        assert_eq!(OutputFormat::Asciidoc.as_str(), "asciidoc");
    }

    #[test]
    fn latex_escape_special_chars() {
        assert_eq!(latex_escape("a & b"), r"a \& b");
        assert_eq!(latex_escape("100%"), r"100\%");
        assert_eq!(latex_escape("a_b"), r"a\_b");
        assert_eq!(latex_escape("$price"), r"\$price");
        assert_eq!(latex_escape("a~b"), r"a\textasciitilde{}b");
    }

    #[test]
    fn markdown_format_produces_pipe_table() {
        use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
        let result = QueryResult {
            columns: vec![Column {
                name: "x".into(),
                type_name: "int4".into(),
                type_oid: 23,
                nullable: false,
            }],
            rows: vec![Row {
                values: vec![CellValue::Int4(42)],
            }],
            affected_rows: None,
            command_tag: "SELECT".into(),
            duration_ms: 1,
        };
        let opts = FormatOptions::default();
        let out = format_result(&result, &OutputFormat::Markdown, &opts).unwrap();
        assert!(out.contains("| x |"), "header: {out}");
        assert!(out.contains("| --- |"), "separator: {out}");
        assert!(out.contains("| 42 |"), "data: {out}");
    }

    #[test]
    fn latex_format_has_tabular_env() {
        use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
        let result = QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
                type_oid: 25,
                nullable: true,
            }],
            rows: vec![Row {
                values: vec![CellValue::Text("hello".into())],
            }],
            affected_rows: None,
            command_tag: "SELECT".into(),
            duration_ms: 1,
        };
        let opts = FormatOptions::default();
        let out = format_result(&result, &OutputFormat::Latex, &opts).unwrap();
        assert!(out.contains(r"\begin{tabular}"), "begin: {out}");
        assert!(out.contains(r"\end{tabular}"), "end: {out}");
        assert!(out.contains("hello"), "data: {out}");
    }

    fn sample_result() -> crate::protocol::messages::QueryResult {
        use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
        QueryResult {
            columns: vec![
                Column {
                    name: "id".into(),
                    type_name: "int4".into(),
                    type_oid: 23,
                    nullable: false,
                },
                Column {
                    name: "name".into(),
                    type_name: "text".into(),
                    type_oid: 25,
                    nullable: true,
                },
            ],
            rows: vec![
                Row {
                    values: vec![CellValue::Int4(1), CellValue::Text("alice".into())],
                },
                Row {
                    values: vec![CellValue::Int4(2), CellValue::Null],
                },
            ],
            affected_rows: None,
            command_tag: "SELECT 2".into(),
            duration_ms: 3,
        }
    }

    #[test]
    fn tsv_format_has_tab_separated_header_and_rows() {
        let result = sample_result();
        let opts = FormatOptions::default();
        let out = format_result(&result, &OutputFormat::Tsv, &opts).unwrap();
        assert!(out.starts_with("id\tname\n"), "header: {out}");
        assert!(out.contains("1\talice\n"), "row: {out}");
    }

    #[test]
    fn tsv_format_tuples_only_omits_header() {
        let result = sample_result();
        let opts = FormatOptions {
            tuples_only: true,
            ..FormatOptions::default()
        };
        let out = format_result(&result, &OutputFormat::Tsv, &opts).unwrap();
        assert!(!out.contains("id\tname"), "header should be absent: {out}");
    }

    #[test]
    fn html_format_escapes_and_wraps_rows() {
        let result = sample_result();
        let opts = FormatOptions::default();
        let out = format_result(&result, &OutputFormat::Html, &opts).unwrap();
        assert!(out.contains("<table>"));
        assert!(out.contains("<th>id</th>"));
        assert!(out.contains("<td>alice</td>"));
        assert!(out.contains("</table>"));
    }

    #[test]
    fn unaligned_format_uses_configured_separators() {
        let result = sample_result();
        let opts = FormatOptions {
            field_separator: ",".to_string(),
            record_separator: ";".to_string(),
            ..FormatOptions::default()
        };
        let out = format_result(&result, &OutputFormat::Unaligned, &opts).unwrap();
        assert!(out.contains("id,name;"), "header: {out}");
        assert!(out.contains("1,alice;"), "row: {out}");
    }

    #[test]
    fn asciidoc_format_has_table_markers() {
        let result = sample_result();
        let opts = FormatOptions::default();
        let out = format_result(&result, &OutputFormat::Asciidoc, &opts).unwrap();
        assert!(out.starts_with("|===\n"));
        assert!(out.trim_end().ends_with("|==="));
        assert!(out.contains("| id"));
        assert!(out.contains("| alice"));
    }

    #[test]
    fn asciidoc_format_uses_null_display_for_nulls() {
        let result = sample_result();
        let opts = FormatOptions {
            null_display: "[NULL]".to_string(),
            ..FormatOptions::default()
        };
        let out = format_result(&result, &OutputFormat::Asciidoc, &opts).unwrap();
        assert!(out.contains("[NULL]"), "expected null placeholder: {out}");
    }

    #[test]
    fn markdown_format_escapes_pipe_and_newline() {
        use crate::protocol::messages::{CellValue, Column, QueryResult, Row};
        let result = QueryResult {
            columns: vec![Column {
                name: "v".into(),
                type_name: "text".into(),
                type_oid: 25,
                nullable: true,
            }],
            rows: vec![Row {
                values: vec![CellValue::Text("a|b\nc".into())],
            }],
            affected_rows: None,
            command_tag: "SELECT".into(),
            duration_ms: 1,
        };
        let opts = FormatOptions::default();
        let out = format_result(&result, &OutputFormat::Markdown, &opts).unwrap();
        assert!(out.contains(r"a\|b<br>c"), "escaped cell: {out}");
    }

    #[test]
    fn colorize_explain_plan_non_terminal_is_passthrough() {
        let rows = vec![
            "Seq Scan on foo".to_string(),
            "  (cost=0.00..1.00)".to_string(),
        ];
        let out = colorize_explain_plan(&rows, false);
        assert_eq!(out, "Seq Scan on foo\n  (cost=0.00..1.00)\n");
    }

    #[test]
    fn colorize_explain_plan_terminal_highlights_seq_scan() {
        let rows = vec!["Seq Scan on foo".to_string()];
        let out = colorize_explain_plan(&rows, true);
        // colored crate wraps in ANSI codes when a terminal is claimed;
        // the underlying text must still be present.
        assert!(out.contains("Seq Scan on foo"));
    }

    #[test]
    fn colorize_explain_plan_handles_actual_time_and_planning_lines() {
        let rows = vec![
            "  ->  Index Scan (actual time=150.123..200.456 rows=1 loops=1)".to_string(),
            "Planning Time: 0.123 ms".to_string(),
            "Execution Time: 1.234 ms".to_string(),
        ];
        let out = colorize_explain_plan(&rows, true);
        assert!(out.contains("Index Scan"));
        assert!(out.contains("Planning Time"));
        assert!(out.contains("Execution Time"));
    }
}
