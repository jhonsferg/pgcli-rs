/// Aligned table renderer using `comfy-table`.
use colored::Colorize;
use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS, presets, Cell, CellAlignment, ColumnConstraint,
    ContentArrangement, Table, Width,
};

use crate::error::Result;
use crate::output::formats::{format_duration, FormatOptions, Formatter, LineStyle};
use crate::protocol::messages::{is_numeric, CellValue, QueryResult};
use serde_json;

/// Formats a `QueryResult` as a bordered table using `comfy-table`.
pub struct TableFormatter;

impl Formatter for TableFormatter {
    fn format(&self, result: &QueryResult, opts: &FormatOptions) -> Result<String> {
        if opts.expanded {
            return Ok(format_expanded(result, opts));
        }
        format_table(result, opts)
    }
}

/// Render `result` as a normal aligned table.
fn format_table(result: &QueryResult, opts: &FormatOptions) -> Result<String> {
    let mut out = String::new();

    // Optional title line printed above the table.
    if let Some(ref title) = opts.title {
        out.push_str(title);
        out.push('\n');
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);

    // Apply border preset based on opts.border and opts.line_style.
    apply_preset(&mut table, opts);

    // Header row - colorize based on theme.
    if !opts.tuples_only {
        let headers: Vec<Cell> = result
            .columns
            .iter()
            .map(|c| {
                let name = match opts.theme.as_str() {
                    "dark" => c.name.bright_cyan().bold().to_string(),
                    "light" => c.name.blue().bold().to_string(),
                    _ => c.name.clone(),
                };
                Cell::new(name)
            })
            .collect();
        table.set_header(headers);

        // Set a minimum column width equal to the plain header text length so
        // that column borders never collapse when there are no data rows.
        for (i, col) in result.columns.iter().enumerate() {
            let min_w = col.name.len().max(1) as u16;
            if let Some(column) = table.column_mut(i) {
                column.set_constraint(ColumnConstraint::LowerBoundary(Width::Fixed(min_w)));
            }
        }
    }

    if result.rows.is_empty() {
        // Insert a centred "(No results)" message row so the user gets visual
        // feedback without an eerily blank table body.
        if !opts.tuples_only && !result.columns.is_empty() {
            let ncols = result.columns.len();
            let mut cells: Vec<Cell> =
                vec![Cell::new("(No results)").set_alignment(CellAlignment::Center)];
            for _ in 1..ncols {
                cells.push(Cell::new(""));
            }
            table.add_row(cells);
        }
    } else {
        // Data rows.
        for row in &result.rows {
            let cells: Vec<Cell> = row
                .values
                .iter()
                .zip(&result.columns)
                .map(|(v, col)| {
                    let mut s = cell_display_typed(v, &opts.null_display, &col.type_name);
                    if opts.numeric_locale && is_numeric(v) {
                        s = apply_numeric_locale(&s);
                    }
                    let s = truncate_cell(&s, opts.max_column_width);
                    let mut cell = Cell::new(s);
                    if is_numeric(v) {
                        cell = cell.set_alignment(CellAlignment::Right);
                    }
                    cell
                })
                .collect();
            table.add_row(cells);
        }
    }

    out.push_str(&table.to_string());

    // Footer: row count + timing (suppressed by tuples_only or footer=false).
    if !opts.tuples_only && opts.footer {
        let row_count = result.rows.len();
        let row_word = if row_count == 1 { "row" } else { "rows" };
        let timing = if opts.timing {
            format!(" - {}", format_duration(result.duration_ms))
        } else {
            String::new()
        };
        out.push_str(&format!("\n({row_count} {row_word}){timing}"));
    }

    Ok(out)
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
        let timing = if opts.timing {
            format!(" - {}", format_duration(result.duration_ms))
        } else {
            String::new()
        };
        out.push_str(&format!("({row_count} {row_word}){timing}"));
    }
    out
}

/// Apply the appropriate `comfy-table` preset for the given options.
fn apply_preset(table: &mut Table, opts: &FormatOptions) {
    match (opts.border, &opts.line_style) {
        (0, _) => {
            table.load_preset(presets::NOTHING);
        }
        (1, LineStyle::Ascii) | (1, LineStyle::OldAscii) => {
            table.load_preset(presets::ASCII_MARKDOWN);
        }
        (2, LineStyle::Ascii) | (2, LineStyle::OldAscii) => {
            table.load_preset(presets::ASCII_FULL);
        }
        (1, _) => {
            table.load_preset(presets::UTF8_FULL);
        }
        (2, _) => {
            table.load_preset(presets::UTF8_FULL);
            table.apply_modifier(UTF8_ROUND_CORNERS);
        }
        _ => {
            table.load_preset(presets::UTF8_FULL);
        }
    }
}

/// Return the display string for a cell value, substituting `null_display` for NULL.
/// For JSON/JSONB columns, pretty-prints the JSON value.
fn cell_display_typed(v: &CellValue, null_display: &str, type_name: &str) -> String {
    match v {
        CellValue::Null => null_display.to_string(),
        other => {
            let s = other.to_string();
            // Pretty-print JSON/JSONB column values when they look like JSON.
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

/// Insert thousands separators into a numeric string (e.g. `"1234567.89"` → `"1,234,567.89"`).
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

/// Truncate a string to `max_width` characters, appending `…` if needed.
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
        assert!(out.contains("(No results)"), "no-results marker missing: {out}");
        assert!(out.contains("(0 rows)"), "row count missing: {out}");
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
