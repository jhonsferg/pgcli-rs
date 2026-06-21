/// Aligned table renderer - pure-Rust implementation with no external dependency
/// on terminal-width detection.
///
/// Column widths are computed from the visible character count of each cell,
/// so ANSI escape codes applied to header text never inflate the column width.
use colored::Colorize;
use serde_json;

use crate::error::Result;
use crate::output::formats::{format_duration, FormatOptions, Formatter, LineStyle};
use crate::protocol::messages::{is_numeric, CellValue, QueryResult};

/// Formats a `QueryResult` as a bordered table.
pub struct TableFormatter;

impl Formatter for TableFormatter {
    fn format(&self, result: &QueryResult, opts: &FormatOptions) -> Result<String> {
        if opts.expanded {
            return Ok(format_expanded(result, opts));
        }
        Ok(format_table(result, opts))
    }
}

/// Box-drawing characters for one border style.
struct Chars {
    v: char,
    h: char,
    hm: char,
    tl: char,
    tm: char,
    tr: char,
    ml: char,
    mm: char,
    mr: char,
    bl: char,
    bm: char,
    br: char,
}

impl Chars {
    fn unicode_plain() -> Self {
        Self {
            v: '│',
            h: '─',
            hm: '─',
            tl: '┌',
            tm: '┬',
            tr: '┐',
            ml: '├',
            mm: '┼',
            mr: '┤',
            bl: '└',
            bm: '┴',
            br: '┘',
        }
    }

    fn unicode_round() -> Self {
        // border=2: rounded corners, double-line header separator
        Self {
            v: '│',
            h: '─',
            hm: '═',
            tl: '╭',
            tm: '┬',
            tr: '╮',
            ml: '╞',
            mm: '╪',
            mr: '╡',
            bl: '╰',
            bm: '┴',
            br: '╯',
        }
    }

    fn ascii() -> Self {
        Self {
            v: '|',
            h: '-',
            hm: '-',
            tl: '+',
            tm: '+',
            tr: '+',
            ml: '+',
            mm: '+',
            mr: '+',
            bl: '+',
            bm: '+',
            br: '+',
        }
    }
}

fn select_chars(opts: &FormatOptions) -> Option<Chars> {
    if opts.border == 0 {
        return None;
    }
    match (&opts.line_style, opts.border) {
        (LineStyle::Ascii | LineStyle::OldAscii, _) => Some(Chars::ascii()),
        (_, 2) => Some(Chars::unicode_round()),
        _ => Some(Chars::unicode_plain()),
    }
}

/// Render a full horizontal rule.
fn hline(widths: &[usize], l: char, m: char, r: char, fill: char) -> String {
    let mut s = String::new();
    s.push(l);
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 {
            s.push(fill);
        }
        if i + 1 < widths.len() {
            s.push(m);
        }
    }
    s.push(r);
    s
}

fn timing_suffix(result: &QueryResult, opts: &FormatOptions) -> String {
    if opts.timing {
        format!(" - {}", format_duration(result.duration_ms))
    } else {
        String::new()
    }
}

/// Render `result` as a normal aligned table.
fn format_table(result: &QueryResult, opts: &FormatOptions) -> String {
    let mut out = String::new();

    if let Some(ref title) = opts.title {
        out.push_str(title);
        out.push('\n');
    }

    let ncols = result.columns.len();
    if ncols == 0 {
        if !opts.tuples_only && opts.footer {
            let timing = timing_suffix(result, opts);
            out.push_str(&format!("(0 rows){timing}"));
        }
        return out;
    }

    // Compute per-column display widths from visible character counts only.
    // ANSI codes in colored header strings are NOT included in the count.
    let col_widths: Vec<usize> = (0..ncols)
        .map(|i| {
            let col = &result.columns[i];
            let hdr_w = col.name.chars().count();
            let data_w = result
                .rows
                .iter()
                .map(|row| {
                    row.values
                        .get(i)
                        .map(|v| {
                            let s = cell_display_typed(v, &opts.null_display, &col.type_name);
                            truncate_cell(&s, opts.max_column_width).chars().count()
                        })
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            hdr_w.max(data_w).max(1)
        })
        .collect();

    let chars = select_chars(opts);

    match &chars {
        Some(c) => {
            // ── Bordered render (border >= 1) ────────────────────────────────
            out.push_str(&hline(&col_widths, c.tl, c.tm, c.tr, c.h));
            out.push('\n');

            if !opts.tuples_only {
                // Header row - pad using plain-text width so ANSI codes do not
                // shift cell borders.
                out.push(c.v);
                for (i, col) in result.columns.iter().enumerate() {
                    let plain_w = col.name.chars().count();
                    let pad = col_widths[i] - plain_w;
                    let colored = match opts.theme.as_str() {
                        "dark" => col.name.bright_cyan().bold().to_string(),
                        "light" => col.name.blue().bold().to_string(),
                        _ => col.name.clone(),
                    };
                    out.push(' ');
                    out.push_str(&colored);
                    for _ in 0..pad {
                        out.push(' ');
                    }
                    out.push(' ');
                    out.push(c.v);
                }
                out.push('\n');

                // Header/body separator.
                out.push_str(&hline(&col_widths, c.ml, c.mm, c.mr, c.hm));
                out.push('\n');
            }

            // Data rows or centred "(No results)" placeholder.
            if result.rows.is_empty() && !opts.tuples_only {
                // Inner width = sum(col_widths) + 2*ncols (padding) + (ncols-1) (separators).
                let inner_w: usize =
                    col_widths.iter().sum::<usize>() + col_widths.len() * 2 + col_widths.len() - 1;
                let msg = "(No results)";
                let msg_w = msg.chars().count();
                let pad_total = inner_w.saturating_sub(msg_w);
                let pad_l = pad_total / 2;
                let pad_r = pad_total - pad_l;
                out.push(c.v);
                for _ in 0..pad_l {
                    out.push(' ');
                }
                out.push_str(msg);
                for _ in 0..pad_r {
                    out.push(' ');
                }
                out.push(c.v);
                out.push('\n');
            } else {
                for row in &result.rows {
                    out.push(c.v);
                    for (i, col) in result.columns.iter().enumerate() {
                        let val = row.values.get(i).unwrap_or(&CellValue::Null);
                        let mut s = cell_display_typed(val, &opts.null_display, &col.type_name);
                        if opts.numeric_locale && is_numeric(val) {
                            s = apply_numeric_locale(&s);
                        }
                        let s = truncate_cell(&s, opts.max_column_width);
                        let vis = s.chars().count();
                        let w = col_widths[i];
                        let pad = w.saturating_sub(vis);
                        if is_numeric(val) {
                            // Right-align numbers.
                            out.push(' ');
                            for _ in 0..pad {
                                out.push(' ');
                            }
                            out.push_str(&s);
                            out.push(' ');
                        } else {
                            out.push(' ');
                            out.push_str(&s);
                            for _ in 0..pad {
                                out.push(' ');
                            }
                            out.push(' ');
                        }
                        out.push(c.v);
                    }
                    out.push('\n');
                }
            }

            // Bottom border.
            out.push_str(&hline(&col_widths, c.bl, c.bm, c.br, c.h));
            out.push('\n');
        }

        None => {
            // ── Border-less render (border = 0) ──────────────────────────────
            // Columns separated by two spaces; no outer delimiters; no lines.
            if !opts.tuples_only {
                // Header.
                for (i, col) in result.columns.iter().enumerate() {
                    let plain_w = col.name.chars().count();
                    let pad = col_widths[i] - plain_w;
                    let colored = match opts.theme.as_str() {
                        "dark" => col.name.bright_cyan().bold().to_string(),
                        "light" => col.name.blue().bold().to_string(),
                        _ => col.name.clone(),
                    };
                    out.push_str(&colored);
                    for _ in 0..pad {
                        out.push(' ');
                    }
                    if i + 1 < ncols {
                        out.push_str("  ");
                    }
                }
                out.push('\n');
                // Separator line (dashes).
                for (i, &w) in col_widths.iter().enumerate() {
                    for _ in 0..w {
                        out.push('-');
                    }
                    if i + 1 < ncols {
                        out.push_str("  ");
                    }
                }
                out.push('\n');
            }

            if result.rows.is_empty() && !opts.tuples_only {
                out.push_str("(No results)\n");
            } else {
                for row in &result.rows {
                    for (i, col) in result.columns.iter().enumerate() {
                        let val = row.values.get(i).unwrap_or(&CellValue::Null);
                        let mut s = cell_display_typed(val, &opts.null_display, &col.type_name);
                        if opts.numeric_locale && is_numeric(val) {
                            s = apply_numeric_locale(&s);
                        }
                        let s = truncate_cell(&s, opts.max_column_width);
                        let vis = s.chars().count();
                        let w = col_widths[i];
                        let pad = w.saturating_sub(vis);
                        if is_numeric(val) {
                            for _ in 0..pad {
                                out.push(' ');
                            }
                            out.push_str(&s);
                        } else {
                            out.push_str(&s);
                            for _ in 0..pad {
                                out.push(' ');
                            }
                        }
                        if i + 1 < ncols {
                            out.push_str("  ");
                        }
                    }
                    out.push('\n');
                }
            }
        }
    }

    // Footer.
    if !opts.tuples_only && opts.footer {
        let row_count = result.rows.len();
        let row_word = if row_count == 1 { "row" } else { "rows" };
        let timing = timing_suffix(result, opts);
        out.push_str(&format!("({row_count} {row_word}){timing}"));
    }

    out
}

/// Render `result` in expanded (vertical) mode: one column per line per row.
fn format_expanded(result: &QueryResult, opts: &FormatOptions) -> String {
    let mut out = String::new();
    for (i, row) in result.rows.iter().enumerate() {
        out.push_str(&format!("-[ RECORD {} ]", i + 1));
        out.push('\n');
        for (col, val) in result.columns.iter().zip(&row.values) {
            let mut display = cell_display_typed(val, &opts.null_display, &col.type_name);
            if opts.numeric_locale && is_numeric(val) {
                display = apply_numeric_locale(&display);
            }
            out.push_str(&format!("{:<20} | {display}\n", col.name));
        }
    }
    if !opts.tuples_only && opts.footer {
        let row_count = result.rows.len();
        let row_word = if row_count == 1 { "row" } else { "rows" };
        let timing = timing_suffix(result, opts);
        out.push_str(&format!("({row_count} {row_word}){timing}"));
    }
    out
}

/// Return the display string for a cell value, substituting `null_display` for NULL.
/// For JSON/JSONB columns, pretty-prints the JSON value.
fn cell_display_typed(v: &CellValue, null_display: &str, type_name: &str) -> String {
    match v {
        CellValue::Null => null_display.to_string(),
        other => {
            let s = other.to_string();
            if matches!(type_name, "json" | "jsonb") && (s.starts_with('{') || s.starts_with('[')) {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let Ok(pretty) = serde_json::to_string_pretty(&val) {
                        return pretty;
                    }
                }
            }
            s
        }
    }
}

/// Insert thousands separators into a numeric string (e.g. `"1234567.89"` -> `"1,234,567.89"`).
///
/// Returns the original string unchanged if it is not a pure integer/decimal.
pub fn apply_numeric_locale(s: &str) -> String {
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let (sign, digits) = if int_part.starts_with('-') || int_part.starts_with('+') {
        (&int_part[..1], &int_part[1..])
    } else {
        ("", int_part)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return s.to_string();
    }
    let with_commas: String = digits
        .chars()
        .rev()
        .enumerate()
        .flat_map(|(i, c)| {
            if i > 0 && i % 3 == 0 {
                vec![',', c]
            } else {
                vec![c]
            }
        })
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    match frac {
        Some(f) => format!("{sign}{with_commas}.{f}"),
        None => format!("{sign}{with_commas}"),
    }
}

/// Truncate a string to `max_width` characters, appending `...` if needed.
/// A `max_width` of 0 means unlimited.
fn truncate_cell(s: &str, max_width: usize) -> String {
    if max_width == 0 || s.chars().count() <= max_width {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_width.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::messages::{Column, Row};

    fn make_result(cols: &[&str], rows: &[Vec<CellValue>]) -> QueryResult {
        QueryResult {
            columns: cols
                .iter()
                .map(|n| Column {
                    name: n.to_string(),
                    type_name: "text".to_string(),
                    type_oid: 25,
                    nullable: true,
                })
                .collect(),
            rows: rows.iter().map(|r| Row { values: r.clone() }).collect(),
            affected_rows: None,
            command_tag: "SELECT".to_string(),
            duration_ms: 5,
        }
    }

    #[test]
    fn table_format_contains_headers() {
        let result = make_result(
            &["id", "name"],
            &[vec![
                CellValue::Int4(1),
                CellValue::Text("Alice".to_string()),
            ]],
        );
        let opts = FormatOptions::default();
        let out = TableFormatter.format(&result, &opts).unwrap();
        assert!(out.contains("id"), "missing 'id' header: {out}");
        assert!(out.contains("name"), "missing 'name' header: {out}");
        assert!(out.contains("Alice"));
        assert!(out.contains("(1 row)"));
    }

    #[test]
    fn table_format_tuples_only_no_header() {
        let result = make_result(&["x"], &[vec![CellValue::Int4(42)]]);
        let opts = FormatOptions {
            tuples_only: true,
            ..FormatOptions::default()
        };
        let out = TableFormatter.format(&result, &opts).unwrap();
        assert!(!out.contains("(1 row)"));
    }

    #[test]
    fn empty_table_shows_no_results_and_zero_rows() {
        let result = make_result(&["Name", "Type", "Owner"], &[]);
        let opts = FormatOptions::default();
        let out = TableFormatter.format(&result, &opts).unwrap();
        assert!(out.contains("Name"), "header Name missing: {out}");
        assert!(out.contains("Type"), "header Type missing: {out}");
        assert!(out.contains("Owner"), "header Owner missing: {out}");
        assert!(
            out.contains("(No results)"),
            "no-results marker missing: {out}"
        );
        assert!(out.contains("(0 rows)"), "row count missing: {out}");
    }

    #[test]
    fn header_column_width_matches_data_not_ansi() {
        // When data in a column is wider than the header, the column must be as
        // wide as the data. Use theme="none" to avoid ANSI codes in the output.
        let result = make_result(
            &["n"],
            &[vec![CellValue::Text("a_long_value_here".to_string())]],
        );
        let opts = FormatOptions {
            theme: "none".to_string(),
            ..FormatOptions::default()
        };
        let out = TableFormatter.format(&result, &opts).unwrap();
        assert!(out.contains("a_long_value_here"), "data missing: {out}");
        // hline pads each column width + 2, so for width=17 we get 19 dashes.
        assert!(
            out.contains("───────────────────"),
            "column not wide enough for data: {out}"
        );
    }

    #[test]
    fn borders_present_for_empty_table_with_long_headers() {
        // Columns should be wide enough to show their headers even with no data rows.
        let result = make_result(&["schema_name", "table_name", "owner"], &[]);
        let opts = FormatOptions::default();
        let out = TableFormatter.format(&result, &opts).unwrap();
        // Each header text must appear intact.
        assert!(out.contains("schema_name"), "schema_name missing: {out}");
        assert!(out.contains("table_name"), "table_name missing: {out}");
        assert!(out.contains("owner"), "owner missing: {out}");
        // Borders must be present (default border=2 uses Unicode).
        assert!(out.contains('│'), "border │ missing: {out}");
    }

    #[test]
    fn truncate_cell_at_limit() {
        let s = truncate_cell("hello world", 5);
        assert_eq!(s.chars().count(), 5);
        assert!(s.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_cell_no_truncate() {
        let s = truncate_cell("hi", 10);
        assert_eq!(s, "hi");
    }

    #[test]
    fn expanded_output_contains_record_marker() {
        let result = make_result(&["id"], &[vec![CellValue::Int4(1)]]);
        let opts = FormatOptions {
            expanded: true,
            ..FormatOptions::default()
        };
        let out = TableFormatter.format(&result, &opts).unwrap();
        assert!(out.contains("-[ RECORD 1 ]"));
    }

    #[test]
    fn apply_numeric_locale_large_integer() {
        assert_eq!(apply_numeric_locale("1000000"), "1,000,000");
        assert_eq!(apply_numeric_locale("1234567890"), "1,234,567,890");
        assert_eq!(apply_numeric_locale("999"), "999");
        assert_eq!(apply_numeric_locale("-1234567"), "-1,234,567");
    }

    #[test]
    fn apply_numeric_locale_with_fraction() {
        assert_eq!(apply_numeric_locale("1234.56"), "1,234.56");
        assert_eq!(apply_numeric_locale("1000000.789"), "1,000,000.789");
    }

    #[test]
    fn apply_numeric_locale_non_numeric_passthrough() {
        assert_eq!(apply_numeric_locale("hello"), "hello");
        assert_eq!(apply_numeric_locale(""), "");
    }

    #[test]
    fn numeric_locale_applied_in_table() {
        let result = make_result(&["n"], &[vec![CellValue::Int8(9_999_999)]]);
        let opts = FormatOptions {
            numeric_locale: true,
            ..FormatOptions::default()
        };
        let out = TableFormatter.format(&result, &opts).unwrap();
        assert!(
            out.contains("9,999,999"),
            "expected thousands separators: {out}"
        );
    }
}
