/// Application-level result types returned by query execution.
///
/// These types wrap the raw `tokio_postgres` results and expose a stable API
/// to the rest of the application without leaking driver internals.
use std::fmt;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

/// The full result of executing one SQL statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Column metadata in declaration order.
    pub columns: Vec<Column>,
    /// Data rows returned by the server.
    pub rows: Vec<Row>,
    /// Number of rows affected (for DML statements). `None` for SELECT.
    pub affected_rows: Option<u64>,
    /// The command tag returned by the server (e.g. `"SELECT 5"`, `"INSERT 0 1"`).
    pub command_tag: String,
    /// Wall-clock time the server took to respond, in milliseconds.
    pub duration_ms: u64,
}

impl QueryResult {
    /// Returns `true` when the result set contains at least one row.
    pub fn has_rows(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Returns the number of columns in the result set.
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }
}

/// Metadata for a single column in a query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    /// Column name as reported by the server.
    pub name: String,
    /// PostgreSQL type name (e.g. `"int4"`, `"text"`, `"timestamp"`).
    pub type_name: String,
    /// PostgreSQL type OID.
    pub type_oid: u32,
    /// Whether the column allows NULL values (`false` = NOT NULL).
    pub nullable: bool,
}

/// A single data row returned from a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    /// Ordered cell values, one per column.
    pub values: Vec<CellValue>,
}

/// A typed cell value from a PostgreSQL result row.
///
/// Covers the full range of PostgreSQL base types. Unknown OIDs fall back
/// to the text representation via `Unknown(String)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CellValue {
    /// SQL NULL.
    Null,
    /// `bool`
    Bool(bool),
    /// `int2` / `smallint`
    Int2(i16),
    /// `int4` / `integer`
    Int4(i32),
    /// `int8` / `bigint`
    Int8(i64),
    /// `float4` / `real`
    Float4(f32),
    /// `float8` / `double precision`
    Float8(f64),
    /// `text`, `varchar`, `char`, `name`, `bpchar`, etc.
    Text(String),
    /// `bytea`-raw bytes.
    Bytea(Vec<u8>),
    /// `uuid`
    Uuid(uuid::Uuid),
    /// `json` / `jsonb`
    Json(serde_json::Value),
    /// `timestamp` (no timezone)
    Timestamp(NaiveDateTime),
    /// `timestamptz` (with timezone, stored as UTC)
    TimestampTz(DateTime<Utc>),
    /// `date`
    Date(NaiveDate),
    /// `time` (no timezone)
    Time(NaiveTime),
    /// `numeric` / `decimal`-stored as its canonical string representation.
    Numeric(String),
    /// `interval`-represented as a human-readable string.
    Interval(String),
    /// PostgreSQL array of any supported element type.
    Array(Vec<CellValue>),
    /// Any type not explicitly handled above, rendered as its text representation.
    Unknown(String),
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, ""),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int2(n) => write!(f, "{n}"),
            Self::Int4(n) => write!(f, "{n}"),
            Self::Int8(n) => write!(f, "{n}"),
            Self::Float4(n) => write!(f, "{n}"),
            Self::Float8(n) => write!(f, "{n}"),
            Self::Text(s) => write!(f, "{s}"),
            Self::Bytea(b) => {
                write!(f, r"\x")?;
                for byte in b {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
            Self::Uuid(u) => write!(f, "{u}"),
            Self::Json(v) => write!(f, "{v}"),
            Self::Timestamp(ts) => write!(f, "{ts}"),
            Self::TimestampTz(ts) => write!(f, "{ts}"),
            Self::Date(d) => write!(f, "{d}"),
            Self::Time(t) => write!(f, "{t}"),
            Self::Numeric(s) => write!(f, "{s}"),
            Self::Interval(s) => write!(f, "{s}"),
            Self::Array(items) => {
                write!(f, "{{")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "}}")
            }
            Self::Unknown(s) => write!(f, "{s}"),
        }
    }
}

/// Indicates whether a `CellValue` holds a numeric type (used for column alignment).
pub fn is_numeric(v: &CellValue) -> bool {
    matches!(
        v,
        CellValue::Int2(_)
            | CellValue::Int4(_)
            | CellValue::Int8(_)
            | CellValue::Float4(_)
            | CellValue::Float8(_)
            | CellValue::Numeric(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_value_null_displays_empty() {
        assert_eq!(CellValue::Null.to_string(), "");
    }

    #[test]
    fn cell_value_int4_display() {
        assert_eq!(CellValue::Int4(42).to_string(), "42");
    }

    #[test]
    fn cell_value_bool_display() {
        assert_eq!(CellValue::Bool(true).to_string(), "true");
    }

    #[test]
    fn cell_value_bytea_hex_display() {
        let val = CellValue::Bytea(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(val.to_string(), r"\xdeadbeef");
    }

    #[test]
    fn cell_value_array_display() {
        let val = CellValue::Array(vec![CellValue::Int4(1), CellValue::Int4(2)]);
        assert_eq!(val.to_string(), "{1,2}");
    }

    #[test]
    fn is_numeric_detects_int_types() {
        assert!(is_numeric(&CellValue::Int4(0)));
        assert!(is_numeric(&CellValue::Float8(0.0)));
        assert!(!is_numeric(&CellValue::Text("x".to_string())));
        assert!(!is_numeric(&CellValue::Null));
    }

    #[test]
    fn query_result_has_rows() {
        let qr = QueryResult {
            columns: vec![],
            rows: vec![Row { values: vec![] }],
            affected_rows: None,
            command_tag: "SELECT 1".to_string(),
            duration_ms: 0,
        };
        assert!(qr.has_rows());
    }

    #[test]
    fn query_result_no_rows_is_false() {
        let qr = QueryResult {
            columns: vec![],
            rows: vec![],
            affected_rows: Some(0),
            command_tag: "UPDATE 0".to_string(),
            duration_ms: 1,
        };
        assert!(!qr.has_rows());
    }

    #[test]
    fn query_result_column_count() {
        let qr = QueryResult {
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
            rows: vec![],
            affected_rows: None,
            command_tag: "SELECT".to_string(),
            duration_ms: 0,
        };
        assert_eq!(qr.column_count(), 2);
    }

    #[test]
    fn cell_value_int2_int8_float_display() {
        assert_eq!(CellValue::Int2(7).to_string(), "7");
        assert_eq!(CellValue::Int8(9_999_999_999).to_string(), "9999999999");
        assert_eq!(CellValue::Float4(1.5).to_string(), "1.5");
    }

    #[test]
    fn cell_value_text_display() {
        assert_eq!(CellValue::Text("hello".to_string()).to_string(), "hello");
    }

    #[test]
    fn cell_value_uuid_display() {
        let u = uuid::Uuid::nil();
        assert_eq!(
            CellValue::Uuid(u).to_string(),
            "00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn cell_value_json_display() {
        let v = serde_json::json!({"a": 1});
        assert_eq!(CellValue::Json(v).to_string(), r#"{"a":1}"#);
    }

    #[test]
    fn cell_value_numeric_and_interval_display() {
        assert_eq!(CellValue::Numeric("3.14".to_string()).to_string(), "3.14");
        assert_eq!(
            CellValue::Interval("1 day".to_string()).to_string(),
            "1 day"
        );
    }

    #[test]
    fn cell_value_date_time_display() {
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        assert_eq!(CellValue::Date(date).to_string(), "2024-01-15");

        let time = NaiveTime::from_hms_opt(13, 30, 0).unwrap();
        assert_eq!(CellValue::Time(time).to_string(), "13:30:00");

        let ts = NaiveDateTime::new(date, time);
        assert_eq!(CellValue::Timestamp(ts).to_string(), "2024-01-15 13:30:00");
    }

    #[test]
    fn cell_value_unknown_display() {
        assert_eq!(
            CellValue::Unknown("custom".to_string()).to_string(),
            "custom"
        );
    }

    #[test]
    fn cell_value_nested_array_display() {
        let inner = CellValue::Array(vec![CellValue::Int4(1), CellValue::Int4(2)]);
        let outer = CellValue::Array(vec![inner, CellValue::Null]);
        assert_eq!(outer.to_string(), "{{1,2},}");
    }
}
