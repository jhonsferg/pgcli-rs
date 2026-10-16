//! `\crosstabview`: pivots a query result into a cross-tab table.

/// Pivot a `QueryResult` into a cross-tab (pivot table) string.
///
/// `col_h` is the column whose values become horizontal headers (columns).
/// `col_v` is the column whose values become the row key (left-most column).
/// `col_d` is the column whose values become cell data.
/// If names are empty, defaults are used: col 0, col 1, col 2.
pub fn crosstab_pivot(
    result: &pgcli_core::protocol::messages::QueryResult,
    col_h: &str,
    col_v: &str,
    col_d: &str,
) -> std::result::Result<String, String> {
    if result.columns.len() < 3 {
        return Err("\\crosstabview requires at least 3 columns in the query result.".to_string());
    }

    let find_col = |name: &str, default: usize| -> usize {
        if name.is_empty() {
            default
        } else {
            result
                .columns
                .iter()
                .position(|c| c.name == name)
                .unwrap_or(default)
        }
    };

    let h_idx = find_col(col_h, 0);
    let v_idx = find_col(col_v, 1);
    let d_idx = find_col(col_d, 2);

    // Collect unique column headers (h values) and row keys (v values).
    use std::collections::{BTreeMap, BTreeSet};
    let mut headers: BTreeSet<String> = BTreeSet::new();
    let mut cells: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

    for row in &result.rows {
        let h_val = row
            .values
            .get(h_idx)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let v_val = row
            .values
            .get(v_idx)
            .map(|v| v.to_string())
            .unwrap_or_default();
        let d_val = row
            .values
            .get(d_idx)
            .map(|v| v.to_string())
            .unwrap_or_default();
        headers.insert(h_val.clone());
        cells.entry(v_val).or_default().insert(h_val, d_val);
    }

    let headers: Vec<String> = headers.into_iter().collect();

    // Build output table.
    let row_key_col = result
        .columns
        .get(v_idx)
        .map(|c| c.name.as_str())
        .unwrap_or("Row");
    let mut out = format!("| {row_key_col}");
    for h in &headers {
        out.push_str(&format!(" | {h}"));
    }
    out.push_str(" |\n| ---");
    for _ in &headers {
        out.push_str(" | ---");
    }
    out.push_str(" |\n");

    for (row_key, row_cells) in &cells {
        out.push_str(&format!("| {row_key}"));
        for h in &headers {
            let cell = row_cells.get(h).map(|s| s.as_str()).unwrap_or("");
            out.push_str(&format!(" | {cell}"));
        }
        out.push_str(" |\n");
    }

    Ok(out)
}
