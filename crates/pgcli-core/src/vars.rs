//! psql-style variable interpolation (`\set name value`, `-v name=value`).
//!
//! Shared by the interactive session and the script runner, so a script
//! behaves the same whether it is typed at the prompt or run with `-f`.

use std::collections::HashMap;

/// Replaces variable references in `sql` with their values:
///
/// - `:'name'` becomes a single-quoted string literal (`'` doubled).
/// - `:"name"` becomes a double-quoted identifier (`"` doubled).
/// - `:name` becomes the raw value.
///
/// Longer names are substituted first, so `:ab` is never read as `:a`
/// followed by `b`. Unknown names are left as they are.
pub fn substitute_variables(sql: &str, variables: &HashMap<String, String>) -> String {
    if variables.is_empty() || !sql.contains(':') {
        return sql.to_string();
    }
    let mut result = sql.to_string();
    let mut pairs: Vec<(&String, &String)> = variables.iter().collect();
    pairs.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    for (name, value) in pairs {
        let quoted_literal = value.replace('\'', "''");
        result = result.replace(&format!(":'{name}'"), &format!("'{quoted_literal}'"));
        let quoted_ident = value.replace('"', "\"\"");
        result = result.replace(&format!(":\"{name}\""), &format!("\"{quoted_ident}\""));
        // Bare form last, so the quoted forms take priority.
        result = result.replace(&format!(":{name}"), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn bare_reference_is_replaced_by_the_raw_value() {
        let v = vars(&[("limit", "10")]);
        assert_eq!(
            substitute_variables("SELECT 1 LIMIT :limit", &v),
            "SELECT 1 LIMIT 10"
        );
    }

    #[test]
    fn single_quoted_reference_becomes_an_escaped_literal() {
        let v = vars(&[("name", "O'Brien")]);
        assert_eq!(
            substitute_variables("SELECT :'name'", &v),
            "SELECT 'O''Brien'"
        );
    }

    #[test]
    fn double_quoted_reference_becomes_an_escaped_identifier() {
        let v = vars(&[("tbl", "my\"table")]);
        assert_eq!(
            substitute_variables("SELECT * FROM :\"tbl\"", &v),
            "SELECT * FROM \"my\"\"table\""
        );
    }

    #[test]
    fn longer_names_win_over_their_prefixes() {
        let v = vars(&[("a", "1"), ("ab", "2")]);
        assert_eq!(substitute_variables("SELECT :ab, :a", &v), "SELECT 2, 1");
    }

    #[test]
    fn unknown_and_absent_references_are_left_alone() {
        let v = vars(&[("x", "1")]);
        assert_eq!(substitute_variables("SELECT :y", &v), "SELECT :y");
        assert_eq!(
            substitute_variables("SELECT 1", &HashMap::new()),
            "SELECT 1"
        );
    }
}
