/// CSV export for `QueryResult`.
use std::io::Write;

use crate::error::{PgCliError, Result};
use crate::protocol::messages::QueryResult;

/// Exports a `QueryResult` to CSV format.
pub struct CsvExporter;

impl CsvExporter {
    /// Write `result` to `writer` in RFC 4180 CSV format.
    ///
    /// The first row is the column header row. NULL values are written as
    /// empty strings. Values containing commas, double-quotes, or newlines
    /// are properly quoted.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if writing fails.
    pub fn export(result: &QueryResult, writer: &mut dyn Write) -> Result<()> {
        let mut wtr = csv::Writer::from_writer(writer);

        // Header.
        let headers: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        wtr.write_record(&headers)
            .map_err(|e| PgCliError::Export(e.to_string()))?;

        // Data rows.
        for row in &result.rows {
            let cells: Vec<String> = row
                .values
                .iter()
                .map(|v| {
                    // NULL → empty string.
                    match v {
                        crate::protocol::messages::CellValue::Null => String::new(),
                        other => other.to_string(),
                    }
                })
                .collect();
            wtr.write_record(&cells)
                .map_err(|e| PgCliError::Export(e.to_string()))?;
        }

        wtr.flush().map_err(|e| PgCliError::Export(e.to_string()))?;
        Ok(())
    }

    /// Export with a configurable NULL display string instead of empty string.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Export` if writing fails.
    pub fn export_with_null(
        result: &QueryResult,
        writer: &mut dyn Write,
        null_display: &str,
    ) -> Result<()> {
        let mut wtr = csv::Writer::from_writer(writer);

        let headers: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
        wtr.write_record(&headers)
            .map_err(|e| PgCliError::Export(e.to_string()))?;

        for row in &result.rows {
            let cells: Vec<String> = row
                .values
                .iter()
                .map(|v| match v {
                    crate::protocol::messages::CellValue::Null => null_display.to_string(),
                    other => other.to_string(),
                })
                .collect();
            wtr.write_record(&cells)
                .map_err(|e| PgCliError::Export(e.to_string()))?;
        }

        wtr.flush().map_err(|e| PgCliError::Export(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::messages::{CellValue, Column, Row};

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
                    values: vec![CellValue::Int4(1), CellValue::Text("Alice".to_string())],
                },
                Row {
                    values: vec![CellValue::Int4(2), CellValue::Null],
                },
            ],
            affected_rows: None,
            command_tag: "SELECT 2".to_string(),
            duration_ms: 1,
        }
    }

    #[test]
    fn csv_has_header() {
        let result = make_result();
        let mut buf = Vec::new();
        CsvExporter::export(&result, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("id,name\n") || s.starts_with("id,name\r\n"));
    }

    #[test]
    fn csv_null_is_empty() {
        let result = make_result();
        let mut buf = Vec::new();
        CsvExporter::export(&result, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Second row, second field should be empty.
        assert!(s.contains("2,\n") || s.contains("2,\r\n") || s.contains("2,\"\""));
    }

    #[test]
    fn csv_null_display_override() {
        let result = make_result();
        let mut buf = Vec::new();
        CsvExporter::export_with_null(&result, &mut buf, "NULL").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("NULL"), "Expected NULL placeholder: {s}");
    }

    #[test]
    fn csv_two_rows() {
        let result = make_result();
        let mut buf = Vec::new();
        CsvExporter::export(&result, &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3); // header + 2 data rows
    }
}
