/// Named query bookmark store.
///
/// Bookmarks are persisted to `~/.pgcli_bookmarks.toml`.
/// Each bookmark is a name → SQL string mapping in a flat TOML table.
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::{PgCliError, Result};

fn bookmarks_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".pgcli_bookmarks.toml"))
        .unwrap_or_else(|| PathBuf::from(".pgcli_bookmarks.toml"))
}

/// Load all bookmarks from `~/.pgcli_bookmarks.toml`.
///
/// Returns an empty map if the file does not exist or cannot be parsed.
pub fn load() -> HashMap<String, String> {
    let path = bookmarks_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    toml::from_str::<toml::Value>(&content)
        .ok()
        .and_then(|v| v.as_table().cloned())
        .map(|t| {
            t.into_iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Save all bookmarks to `~/.pgcli_bookmarks.toml`.
///
/// # Errors
///
/// Returns `PgCliError::Config` if serialization fails, `PgCliError::Io` on write failure.
pub fn save(bookmarks: &HashMap<String, String>) -> Result<()> {
    let mut table = toml::map::Map::new();
    let mut keys: Vec<&String> = bookmarks.keys().collect();
    keys.sort();
    for k in keys {
        table.insert(k.clone(), toml::Value::String(bookmarks[k].clone()));
    }
    let content = toml::to_string(&toml::Value::Table(table))
        .map_err(|e| PgCliError::Config(e.to_string()))?;
    std::fs::write(bookmarks_path(), content).map_err(PgCliError::Io)
}

/// Format the bookmark list as a human-readable string.
pub fn format_list(bookmarks: &HashMap<String, String>) -> String {
    if bookmarks.is_empty() {
        return "No bookmarks saved. Use \\bookmark NAME to save the last query.".to_string();
    }
    let mut keys: Vec<&String> = bookmarks.keys().collect();
    keys.sort();
    keys.iter()
        .map(|k| {
            let sql = &bookmarks[*k];
            let preview = if sql.len() > 60 {
                format!("{}...", &sql[..60])
            } else {
                sql.clone()
            };
            format!("{k:20}  {preview}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_list_empty() {
        let bm: HashMap<String, String> = HashMap::new();
        assert!(format_list(&bm).contains("No bookmarks"));
    }

    #[test]
    fn format_list_truncates_long_sql() {
        let mut bm = HashMap::new();
        bm.insert("long".to_string(), "x".repeat(80));
        let out = format_list(&bm);
        assert!(out.contains("..."));
    }

    #[test]
    fn format_list_sorts_alphabetically() {
        let mut bm = HashMap::new();
        bm.insert("z_query".to_string(), "SELECT 1".to_string());
        bm.insert("a_query".to_string(), "SELECT 2".to_string());
        let out = format_list(&bm);
        let z_pos = out.find("z_query").unwrap();
        let a_pos = out.find("a_query").unwrap();
        assert!(a_pos < z_pos);
    }
}
