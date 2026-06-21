/// JSON and JSON Lines export for `QueryResult`.
use std::io::Write;

use serde_json::{json, Value};

use crate::error::{PgCliError, Result};
use crate::protocol::messages::{CellValue, QueryResult};

/// Exports a `QueryResult` to JSON or JSON Lines format.
pub struct JsonExporter;

impl JsonExporter {
    /// Write `result` as a JSON array of objects to `writer`.
    ///
    /// Each row becomes a JSON object with column names as keys.
    /// NULL values become JSON `null`. Numeric types become JSON numbers.
    /// Timestamps become ISO 8601 strings.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if serialization or writing fails.
    pub fn export_array(result: &QueryResult, writer: &mut dyn Write) -> Result<()> {
        let array: Vec<Value> = result
            .rows
            .iter()
            .map(|row| {
                let mut obj = serde_json::Map::new();
                for (col, val) in result.columns.iter().zip(&row.values) {
                    obj.insert(col.name.clone(), cell_to_json(val));
                }
                Value::Object(obj)
            })
            .collect();

        serde_json::to_writer_pretty(&mut *writer, &Value::Array(array))
            .map_err(|e| PgCliError::Export(e.to_string()))?;
        writer
            .write_all(b"\n")
            .map_err(|e| PgCliError::Export(e.to_string()))?;
        Ok(())
    }

    /// Write `result` as JSON Lines (NDJSON) to `writer`.
    ///
    /// Each row is a separate JSON object on its own line.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if serialization or writing fails.
    pub fn export_lines(result: &QueryResult, writer: &mut dyn Write) -> Result<()> {
        for row in &result.rows {
            let mut obj = serde_json::Map::new();
            for (col, val) in result.columns.iter().zip(&row.values) {
                obj.insert(col.name.clone(), cell_to_json(val));
            }
            let line = serde_json::to_string(&Value::Object(obj))
                .map_err(|e| PgCliError::Export(e.to_string()))?;
            writer
                .write_all(line.as_bytes())
                .map_err(|e| PgCliError::Export(e.to_string()))?;
            writer
                .write_all(b"\n")
                .map_err(|e| PgCliError::Export(e.to_string()))?;
        }
        Ok(())
    }
}

/// Convert a `CellValue` to a `serde_json::Value`.
fn cell_to_json(v: &CellValue) -> Value {
    match v {
        CellValue::Null => Value::Null,
        CellValue::Bool(b) => json!(b),
        CellValue::Int2(n) => json!(n),
        CellValue::Int4(n) => json!(n),
        CellValue::Int8(n) => json!(n),
        CellValue::Float4(f) => {
            if f.is_nan() || f.is_infinite() {
                Value::Null
            } else {
                json!(f)
            }
        }
        CellValue::Float8(f) => {
            if f.is_nan() || f.is_infinite() {
                Value::Null
            } else {
                json!(f)
            }
        }
        CellValue::Text(s) => json!(s),
        CellValue::Bytea(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            json!(format!("\\x{hex}"))
        }
        CellValue::Uuid(u) => json!(u.to_string()),
        CellValue::Json(v) => v.clone(),
        CellValue::Timestamp(ts) => json!(ts.to_string()),
        CellValue::TimestampTz(ts) => json!(ts.to_rfc3339()),
        CellValue::Date(d) => json!(d.to_string()),
        CellValue::Time(t) => json!(t.to_string()),
        CellValue::Numeric(s) => json!(s),
        CellValue::Interval(s) => json!(s),
        CellValue::Array(items) => Value::Array(items.iter().map(cell_to_json).collect()),
        CellValue::Unknown(s) => json!(s),
    }
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
                    name: "val".to_string(),
                    type_name: "text".to_string(),
                    type_oid: 25,
                    nullable: true,
                },
            ],
            rows: vec![
                Row {
                    values: vec![CellValue::Int4(1), CellValue::Text("hello".to_string())],
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
    fn json_array_is_valid() {
        let result = make_result();
        let mut buf = Vec::new();
        JsonExporter::export_array(&result, &mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert!(v.is_array());
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn json_null_becomes_json_null() {
        let result = make_result();
        let mut buf = Vec::new();
        JsonExporter::export_array(&result, &mut buf).unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v[1]["val"], Value::Null);
    }

    #[test]
    fn jsonl_each_row_is_separate_line() {
        let result = make_result();
        let mut buf = Vec::new();
        JsonExporter::export_lines(&result, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2);
        let v0: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v0["id"], json!(1));
    }

    #[test]
    fn cell_nan_float_becomes_null() {
        let v = cell_to_json(&CellValue::Float8(f64::NAN));
        assert_eq!(v, Value::Null);
    }
}
