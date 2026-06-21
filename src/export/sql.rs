/// SQL INSERT and COPY export for `QueryResult`.
use std::io::Write;

use crate::error::{PgCliError, Result};
use crate::protocol::messages::{CellValue, QueryResult};

/// Exports a `QueryResult` as SQL INSERT statements or COPY format.
pub struct SqlExporter;

impl SqlExporter {
    /// Write `result` as a series of `INSERT INTO` statements.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if writing fails.
    pub fn export_insert(
        result: &QueryResult,
        table_name: &str,
        writer: &mut dyn Write,
    ) -> Result<()> {
        if result.rows.is_empty() {
            return Ok(());
        }

        let col_list: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        let col_str = col_list.join(", ");

        for row in &result.rows {
            let values: Vec<String> = row.values.iter().map(sql_literal).collect();
            let val_str = values.join(", ");
            writeln!(
                writer,
                "INSERT INTO {table_name} ({col_str}) VALUES ({val_str});"
            )
            .map_err(|e| PgCliError::Export(e.to_string()))?;
        }

        Ok(())
    }

    /// Write `result` in PostgreSQL `COPY ... FROM stdin` format.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if writing fails.
    pub fn export_copy(
        result: &QueryResult,
        table_name: &str,
        writer: &mut dyn Write,
    ) -> Result<()> {
        let col_list: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        let col_str = col_list.join(", ");

        writeln!(writer, "COPY {table_name} ({col_str}) FROM stdin;")
            .map_err(|e| PgCliError::Export(e.to_string()))?;

        for row in &result.rows {
            let fields: Vec<String> = row.values.iter().map(copy_literal).collect();
            writeln!(writer, "{}", fields.join("\t"))
                .map_err(|e| PgCliError::Export(e.to_string()))?;
        }

        writeln!(writer, r"\.").map_err(|e| PgCliError::Export(e.to_string()))?;

        Ok(())
    }
}

/// Convert a `CellValue` to a SQL literal suitable for INSERT statements.
fn sql_literal(v: &CellValue) -> String {
    match v {
        CellValue::Null => "NULL".to_string(),
        CellValue::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        CellValue::Int2(n) => n.to_string(),
        CellValue::Int4(n) => n.to_string(),
        CellValue::Int8(n) => n.to_string(),
        CellValue::Float4(f) => f.to_string(),
        CellValue::Float8(f) => f.to_string(),
        CellValue::Text(s) => format!("'{}'", escape_sql_string(s)),
        CellValue::Bytea(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("'\\x{hex}'::bytea")
        }
        CellValue::Uuid(u) => format!("'{u}'"),
        CellValue::Json(v) => format!("'{}'::jsonb", escape_sql_string(&v.to_string())),
        CellValue::Timestamp(ts) => format!("'{ts}'::timestamp"),
        CellValue::TimestampTz(ts) => format!("'{}'::timestamptz", ts.to_rfc3339()),
        CellValue::Date(d) => format!("'{d}'::date"),
        CellValue::Time(t) => format!("'{t}'::time"),
        CellValue::Numeric(s) => s.clone(),
        CellValue::Interval(s) => format!("'{}'::interval", escape_sql_string(s)),
        CellValue::Array(items) => {
            let inner: Vec<String> = items.iter().map(sql_literal).collect();
            format!("ARRAY[{}]", inner.join(","))
        }
        CellValue::Unknown(s) => format!("'{}'", escape_sql_string(s)),
    }
}

/// Convert a `CellValue` to a tab-delimited COPY field.
fn copy_literal(v: &CellValue) -> String {
    match v {
        CellValue::Null => "\\N".to_string(),
        other => {
            // Escape tab, newline, backslash per COPY text format spec.
            other
                .to_string()
                .replace('\\', "\\\\")
                .replace('\t', "\\t")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
        }
    }
}

/// Escape single quotes in SQL string literals by doubling them.
fn escape_sql_string(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::messages::{Column, Row};

    fn make_result() -> QueryResult {
        QueryResult {
            columns: vec![
                Column {
                    name: "id".to_string(),
                    type_name: "int4".to_string(),
                    type_oid: 23,
                    nullable: false,
                },
                Column {
                    name: "name".to_string(),
                    type_name: "text".to_string(),
                    type_oid: 25,
                    nullable: true,
                },
            ],
            rows: vec![
                Row {
                    values: vec![CellValue::Int4(1), CellValue::Text("Alice's".to_string())],
                },
                Row {
                    values: vec![CellValue::Int4(2), CellValue::Null],
                },
            ],
            affected_rows: None,
            command_tag: "SELECT 2".to_string(),
            duration_ms: 0,
        }
    }

    #[test]
    fn insert_escapes_single_quotes() {
        let result = make_result();
        let mut buf = Vec::new();
        SqlExporter::export_insert(&result, "users", &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Alice''s"), "Expected escaped quote: {s}");
    }

    #[test]
    fn insert_null_literal() {
        let result = make_result();
        let mut buf = Vec::new();
        SqlExporter::export_insert(&result, "users", &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("NULL"), "Expected NULL literal: {s}");
    }

    #[test]
    fn copy_format_structure() {
        let result = make_result();
        let mut buf = Vec::new();
        SqlExporter::export_copy(&result, "users", &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("COPY users"));
        assert!(s.contains(r"\."));
        assert!(s.contains(r"\N")); // NULL representation
    }

    #[test]
    fn sql_literal_bool() {
        assert_eq!(sql_literal(&CellValue::Bool(true)), "TRUE");
        assert_eq!(sql_literal(&CellValue::Bool(false)), "FALSE");
    }

    #[test]
    fn escape_sql_string_doubles_quotes() {
        assert_eq!(escape_sql_string("it's"), "it''s");
    }
}
