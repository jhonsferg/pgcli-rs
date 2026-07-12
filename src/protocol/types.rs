/// PostgreSQL OID-to-Rust type mapping.
///
/// Maps well-known PostgreSQL type OIDs to their canonical names and
/// to the corresponding `CellValue` variant used during row decoding.
use crate::protocol::messages::CellValue;
use tokio_postgres::types::Type;
use tokio_postgres::Row as PgRow;

// Well-known OID constants mirroring pg_catalog.pg_type.
/// OID for `bool`
pub const OID_BOOL: u32 = 16;
/// OID for `bytea`
pub const OID_BYTEA: u32 = 17;
/// OID for `int8` / `bigint`
pub const OID_INT8: u32 = 20;
/// OID for `int2` / `smallint`
pub const OID_INT2: u32 = 21;
/// OID for `int4` / `integer`
pub const OID_INT4: u32 = 23;
/// OID for `text`
pub const OID_TEXT: u32 = 25;
/// OID for `oid`
pub const OID_OID: u32 = 26;
/// OID for `float4` / `real`
pub const OID_FLOAT4: u32 = 700;
/// OID for `float8` / `double precision`
pub const OID_FLOAT8: u32 = 701;
/// OID for `bpchar` (blank-padded char)
pub const OID_BPCHAR: u32 = 1042;
/// OID for `varchar`
pub const OID_VARCHAR: u32 = 1043;
/// OID for `date`
pub const OID_DATE: u32 = 1082;
/// OID for `time`
pub const OID_TIME: u32 = 1083;
/// OID for `timestamp`
pub const OID_TIMESTAMP: u32 = 1114;
/// OID for `timestamptz`
pub const OID_TIMESTAMPTZ: u32 = 1184;
/// OID for `interval`
pub const OID_INTERVAL: u32 = 1186;
/// OID for `numeric`
pub const OID_NUMERIC: u32 = 1700;
/// OID for `uuid`
pub const OID_UUID: u32 = 2950;
/// OID for `json`
pub const OID_JSON: u32 = 114;
/// OID for `jsonb`
pub const OID_JSONB: u32 = 3802;

/// Return the canonical PostgreSQL type name for a well-known OID.
///
/// Falls back to `"unknown"` for unrecognized OIDs.
pub fn oid_to_name(oid: u32) -> &'static str {
    match oid {
        OID_BOOL => "bool",
        OID_BYTEA => "bytea",
        OID_INT8 => "int8",
        OID_INT2 => "int2",
        OID_INT4 => "int4",
        OID_OID => "oid",
        OID_TEXT => "text",
        OID_FLOAT4 => "float4",
        OID_FLOAT8 => "float8",
        OID_BPCHAR => "bpchar",
        OID_VARCHAR => "varchar",
        OID_DATE => "date",
        OID_TIME => "time",
        OID_TIMESTAMP => "timestamp",
        OID_TIMESTAMPTZ => "timestamptz",
        OID_INTERVAL => "interval",
        OID_NUMERIC => "numeric",
        OID_UUID => "uuid",
        OID_JSON => "json",
        OID_JSONB => "jsonb",
        _ => "unknown",
    }
}

/// Extract a `CellValue` from a `tokio_postgres::Row` at column index `idx`.
///
/// Uses the column's `tokio_postgres::types::Type` to select the correct
/// `get::<_, T>()` overload and wraps the result in the appropriate variant.
///
/// NULL values at any column return `CellValue::Null`.
pub fn extract_cell(row: &PgRow, idx: usize) -> CellValue {
    let col_type = row.columns()[idx].type_();

    match col_type {
        &Type::BOOL => row
            .try_get::<_, Option<bool>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Bool)
            .unwrap_or(CellValue::Null),

        &Type::INT2 => row
            .try_get::<_, Option<i16>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Int2)
            .unwrap_or(CellValue::Null),

        &Type::INT4 => row
            .try_get::<_, Option<i32>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Int4)
            .unwrap_or(CellValue::Null),

        &Type::OID => row
            .try_get::<_, Option<u32>>(idx)
            .ok()
            .flatten()
            .map(|n| CellValue::Int8(n as i64))
            .unwrap_or(CellValue::Null),

        &Type::INT8 => row
            .try_get::<_, Option<i64>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Int8)
            .unwrap_or(CellValue::Null),

        &Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Float4)
            .unwrap_or(CellValue::Null),

        &Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Float8)
            .unwrap_or(CellValue::Null),

        &Type::TEXT | &Type::VARCHAR | &Type::BPCHAR | &Type::NAME => row
            .try_get::<_, Option<String>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Text)
            .unwrap_or(CellValue::Null),

        &Type::BYTEA => row
            .try_get::<_, Option<Vec<u8>>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Bytea)
            .unwrap_or(CellValue::Null),

        &Type::UUID => row
            .try_get::<_, Option<uuid::Uuid>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Uuid)
            .unwrap_or(CellValue::Null),

        &Type::JSON | &Type::JSONB => row
            .try_get::<_, Option<serde_json::Value>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Json)
            .unwrap_or(CellValue::Null),

        &Type::TIMESTAMP => row
            .try_get::<_, Option<chrono::NaiveDateTime>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Timestamp)
            .unwrap_or(CellValue::Null),

        &Type::TIMESTAMPTZ => row
            .try_get::<_, Option<chrono::DateTime<chrono::Utc>>>(idx)
            .ok()
            .flatten()
            .map(CellValue::TimestampTz)
            .unwrap_or(CellValue::Null),

        &Type::DATE => row
            .try_get::<_, Option<chrono::NaiveDate>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Date)
            .unwrap_or(CellValue::Null),

        &Type::TIME => row
            .try_get::<_, Option<chrono::NaiveTime>>(idx)
            .ok()
            .flatten()
            .map(CellValue::Time)
            .unwrap_or(CellValue::Null),

        &Type::INTERVAL => row
            .try_get::<_, Option<IntervalStr>>(idx)
            .ok()
            .flatten()
            .map(|iv| CellValue::Interval(iv.0))
            .unwrap_or(CellValue::Null),

        &Type::NUMERIC => row
            .try_get::<_, Option<NumericStr>>(idx)
            .ok()
            .flatten()
            .map(|n| CellValue::Numeric(n.0))
            .unwrap_or(CellValue::Null),

        _ => {
            // Fall back to AnyText for enums, domains, and other custom types.
            // PostgreSQL sends enum labels and text-domain values as UTF-8 bytes;
            // for binary types (inet, point, …) this produces a best-effort
            // representation rather than an empty cell.
            row.try_get::<_, Option<AnyText>>(idx)
                .ok()
                .flatten()
                .map(|t| CellValue::Unknown(t.0))
                .unwrap_or(CellValue::Null)
        }
    }
}

// -- Any-type text fallback --------------------------------------------------
//
// Used in the `_` arm of extract_cell for enum, domain, and other text-based
// custom types whose OID is not matched by `String::from_sql` (which only
// accepts the built-in text OIDs).  PostgreSQL always sends enum labels and
// text-domain values as plain UTF-8 bytes, so we can safely decode them here.

struct AnyText(String);

impl<'a> postgres_types::FromSql<'a> for AnyText {
    fn from_sql(
        _ty: &postgres_types::Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(AnyText(String::from_utf8_lossy(raw).into_owned()))
    }

    fn accepts(_ty: &postgres_types::Type) -> bool {
        true
    }
}

// -- PostgreSQL INTERVAL binary decoder ---------------------------------------
//
// Binary layout: time(i64 µs) + day(i32) + month(i32) = 16 bytes.

/// Newtype that decodes the PostgreSQL INTERVAL binary wire format.
struct IntervalStr(String);

impl<'a> postgres_types::FromSql<'a> for IntervalStr {
    fn from_sql(
        _ty: &postgres_types::Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(IntervalStr(decode_pg_interval(raw)))
    }

    fn accepts(ty: &postgres_types::Type) -> bool {
        ty == &postgres_types::Type::INTERVAL
    }
}

/// Decode a PostgreSQL INTERVAL binary value (16 bytes) into a human-readable string.
fn decode_pg_interval(raw: &[u8]) -> String {
    if raw.len() < 16 {
        return String::new();
    }
    let micros = i64::from_be_bytes([
        raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
    ]);
    let days = i32::from_be_bytes([raw[8], raw[9], raw[10], raw[11]]);
    let months = i32::from_be_bytes([raw[12], raw[13], raw[14], raw[15]]);

    let years = months / 12;
    let rem_months = months % 12;
    let secs = micros / 1_000_000;
    let rem_micros = micros.abs() % 1_000_000;
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    let mut parts: Vec<String> = Vec::new();
    if years != 0 {
        parts.push(format!(
            "{years} year{}",
            if years.abs() != 1 { "s" } else { "" }
        ));
    }
    if rem_months != 0 {
        parts.push(format!(
            "{rem_months} mon{}",
            if rem_months.abs() != 1 { "s" } else { "" }
        ));
    }
    if days != 0 {
        parts.push(format!(
            "{days} day{}",
            if days.abs() != 1 { "s" } else { "" }
        ));
    }
    if hours != 0 || minutes != 0 || seconds != 0 || rem_micros != 0 {
        if rem_micros != 0 {
            parts.push(format!(
                "{hours:02}:{minutes:02}:{seconds:02}.{rem_micros:06}"
            ));
        } else {
            parts.push(format!("{hours:02}:{minutes:02}:{seconds:02}"));
        }
    }

    if parts.is_empty() {
        "00:00:00".to_string()
    } else {
        parts.join(" ")
    }
}

// -- PostgreSQL NUMERIC binary decoder ----------------------------------------
//
// PostgreSQL sends NUMERIC as a binary struct:
//   ndigits : i16  -number of base-10000 groups
//   weight  : i16  -exponent of first group (0 = units, 1 = ten-thousands …)
//   sign    : u16  -0x0000 pos | 0x4000 neg | 0xC000 NaN | 0xD000 Inf
//   dscale  : i16  -number of decimal digits after the point
//   groups  : [u16; ndigits] -each in 0..=9999

/// Newtype that decodes the PostgreSQL NUMERIC binary wire format.
struct NumericStr(String);

impl<'a> postgres_types::FromSql<'a> for NumericStr {
    fn from_sql(
        _ty: &postgres_types::Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(NumericStr(decode_pg_numeric(raw)))
    }

    fn accepts(ty: &postgres_types::Type) -> bool {
        ty == &postgres_types::Type::NUMERIC
    }
}

/// Decode a PostgreSQL NUMERIC binary value into a decimal string.
fn decode_pg_numeric(raw: &[u8]) -> String {
    if raw.len() < 8 {
        return String::new();
    }

    let ndigits = i16::from_be_bytes([raw[0], raw[1]]) as usize;
    let weight = i16::from_be_bytes([raw[2], raw[3]]) as i32;
    let sign = u16::from_be_bytes([raw[4], raw[5]]);
    let dscale = i16::from_be_bytes([raw[6], raw[7]]) as usize;

    match sign {
        0xC000 => return "NaN".to_string(),
        0xD000 => return "Infinity".to_string(),
        0xF000 => return "-Infinity".to_string(),
        _ => {}
    }

    // Collect base-10000 groups.
    let mut groups: Vec<u16> = Vec::with_capacity(ndigits);
    for i in 0..ndigits {
        let off = 8 + i * 2;
        if off + 1 >= raw.len() {
            return String::new();
        }
        groups.push(u16::from_be_bytes([raw[off], raw[off + 1]]));
    }

    // Build integer part.
    let mut int_str = String::new();
    if weight < 0 {
        int_str.push('0');
    } else {
        let int_count = ((weight + 1) as usize).min(ndigits);
        for (i, &g) in groups[..int_count].iter().enumerate() {
            if i == 0 {
                int_str.push_str(&g.to_string());
            } else {
                int_str.push_str(&format!("{g:04}"));
            }
        }
        // Trailing zero groups not stored (e.g., 10000 stored as [1] with weight=1).
        for _ in ndigits..=(weight as usize) {
            if int_str.is_empty() {
                int_str.push('0');
            } else {
                int_str.push_str("0000");
            }
        }
        if int_str.is_empty() {
            int_str.push('0');
        }
    }

    // Build fractional part.
    let mut frac_str = String::new();
    if dscale > 0 {
        // Leading zero groups when weight < -1 (e.g., 0.00001234 has weight=-2).
        if weight < -1 {
            let leading = (-weight - 1) as usize;
            for _ in 0..leading {
                frac_str.push_str("0000");
            }
        }
        let frac_start = if weight >= 0 {
            ((weight + 1) as usize).min(ndigits)
        } else {
            0
        };
        for &g in &groups[frac_start..] {
            frac_str.push_str(&format!("{g:04}"));
        }
        while frac_str.len() < dscale {
            frac_str.push('0');
        }
        frac_str.truncate(dscale);
    }

    let mut result = if sign == 0x4000 {
        format!("-{int_str}")
    } else {
        int_str
    };
    if dscale > 0 {
        result.push('.');
        result.push_str(&frac_str);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oid_to_name_known() {
        assert_eq!(oid_to_name(OID_INT4), "int4");
        assert_eq!(oid_to_name(OID_TEXT), "text");
        assert_eq!(oid_to_name(OID_UUID), "uuid");
        assert_eq!(oid_to_name(OID_TIMESTAMPTZ), "timestamptz");
    }

    #[test]
    fn oid_to_name_unknown() {
        assert_eq!(oid_to_name(99999), "unknown");
    }

    // Helper: build a PostgreSQL NUMERIC binary payload.
    fn make_numeric(ndigits: i16, weight: i16, sign: u16, dscale: i16, groups: &[u16]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ndigits.to_be_bytes());
        v.extend_from_slice(&weight.to_be_bytes());
        v.extend_from_slice(&sign.to_be_bytes());
        v.extend_from_slice(&dscale.to_be_bytes());
        for &g in groups {
            v.extend_from_slice(&g.to_be_bytes());
        }
        v
    }

    #[test]
    fn numeric_decode_integer() {
        // 200 → ndigits=1, weight=0, sign=0, dscale=0, groups=[200]
        let b = make_numeric(1, 0, 0, 0, &[200]);
        assert_eq!(decode_pg_numeric(&b), "200");
    }

    #[test]
    fn numeric_decode_decimal() {
        // 1.50 → ndigits=2, weight=0, sign=0, dscale=2, groups=[1, 5000]
        let b = make_numeric(2, 0, 0, 2, &[1, 5000]);
        assert_eq!(decode_pg_numeric(&b), "1.50");
    }

    #[test]
    fn numeric_decode_200_99() {
        // 200.99 → ndigits=2, weight=0, sign=0, dscale=2, groups=[200, 9900]
        let b = make_numeric(2, 0, 0, 2, &[200, 9900]);
        assert_eq!(decode_pg_numeric(&b), "200.99");
    }

    #[test]
    fn numeric_decode_negative() {
        // -42.50 → ndigits=2, weight=0, sign=0x4000, dscale=2, groups=[42, 5000]
        let b = make_numeric(2, 0, 0x4000, 2, &[42, 5000]);
        assert_eq!(decode_pg_numeric(&b), "-42.50");
    }

    #[test]
    fn numeric_decode_nan() {
        let b = make_numeric(0, 0, 0xC000, 0, &[]);
        assert_eq!(decode_pg_numeric(&b), "NaN");
    }

    #[test]
    fn numeric_decode_large_integer() {
        // 10000 → ndigits=1, weight=1, sign=0, dscale=0, groups=[1]
        let b = make_numeric(1, 1, 0, 0, &[1]);
        assert_eq!(decode_pg_numeric(&b), "10000");
    }

    fn make_interval(micros: i64, days: i32, months: i32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&micros.to_be_bytes());
        v.extend_from_slice(&days.to_be_bytes());
        v.extend_from_slice(&months.to_be_bytes());
        v
    }

    #[test]
    fn interval_zero() {
        let b = make_interval(0, 0, 0);
        assert_eq!(decode_pg_interval(&b), "00:00:00");
    }

    #[test]
    fn interval_one_year_two_months_three_days() {
        // '1 year 2 months 3 days'
        let b = make_interval(0, 3, 14); // 14 months = 1 year 2 months
        assert_eq!(decode_pg_interval(&b), "1 year 2 mons 3 days");
    }

    #[test]
    fn interval_time_only() {
        // '02:30:00'
        let micros = (2 * 3600 + 30 * 60) as i64 * 1_000_000;
        let b = make_interval(micros, 0, 0);
        assert_eq!(decode_pg_interval(&b), "02:30:00");
    }

    #[test]
    fn interval_with_microseconds() {
        // '00:00:01.500000'
        let b = make_interval(1_500_000, 0, 0);
        assert_eq!(decode_pg_interval(&b), "00:00:01.500000");
    }

    #[test]
    fn interval_short_raw_returns_empty() {
        assert_eq!(decode_pg_interval(&[0u8; 4]), "");
    }

    #[test]
    fn interval_single_year_and_month_are_singular() {
        let b = make_interval(0, 0, 13); // 1 year, 1 month
        assert_eq!(decode_pg_interval(&b), "1 year 1 mon");
    }

    #[test]
    fn interval_single_day_is_singular() {
        let b = make_interval(0, 1, 0);
        assert_eq!(decode_pg_interval(&b), "1 day");
    }

    #[test]
    fn numeric_decode_infinity() {
        let b = make_numeric(0, 0, 0xD000, 0, &[]);
        assert_eq!(decode_pg_numeric(&b), "Infinity");
    }

    #[test]
    fn numeric_decode_negative_infinity() {
        let b = make_numeric(0, 0, 0xF000, 0, &[]);
        assert_eq!(decode_pg_numeric(&b), "-Infinity");
    }

    #[test]
    fn numeric_decode_zero() {
        let b = make_numeric(0, 0, 0, 0, &[]);
        assert_eq!(decode_pg_numeric(&b), "0");
    }

    #[test]
    fn numeric_decode_short_raw_returns_empty() {
        assert_eq!(decode_pg_numeric(&[0u8; 4]), "");
    }

    #[test]
    fn numeric_decode_truncated_groups_returns_empty() {
        // Header claims 2 digit groups but only provides 1.
        let mut b = make_numeric(2, 0, 0, 0, &[200]);
        b.truncate(10); // header (8) + 1 group (2) = 10 bytes, missing the 2nd group
        assert_eq!(decode_pg_numeric(&b), "");
    }

    #[test]
    fn numeric_decode_small_fraction_with_leading_zeros() {
        // 0.00001234 → ndigits=1, weight=-2, sign=0, dscale=8, groups=[1234]
        let b = make_numeric(1, -2, 0, 8, &[1234]);
        assert_eq!(decode_pg_numeric(&b), "0.00001234");
    }

    #[test]
    fn oid_constants_map_to_expected_names() {
        assert_eq!(oid_to_name(OID_BOOL), "bool");
        assert_eq!(oid_to_name(OID_TEXT), "text");
        assert_eq!(oid_to_name(OID_JSONB), "jsonb");
        assert_eq!(oid_to_name(OID_UUID), "uuid");
    }
}
