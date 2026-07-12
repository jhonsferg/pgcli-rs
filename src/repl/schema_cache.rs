/// Live schema-object cache for schema-aware tab completion.
///
/// Populated once after connecting and refreshed on `\c` reconnect.
/// Uses `std::sync::RwLock` so the sync `Completer` trait can read it
/// without blocking the async runtime.
use std::sync::{Arc, RwLock};

use tokio_postgres::Client;

use crate::error::Result;

/// Shared, thread-safe handle to the schema cache.
pub type SharedSchemaCache = Arc<RwLock<SchemaCache>>;

/// Cached schema objects used for tab-completion suggestions.
#[derive(Debug, Default)]
pub struct SchemaCache {
    /// Plain table / view / materialized-view names (not schema-qualified).
    pub table_names: Vec<String>,
    /// Schema-qualified names: `"schema.table"`.
    pub qualified_tables: Vec<String>,
    /// Schema names.
    pub schemas: Vec<String>,
    /// Distinct column names across all user tables.
    pub columns: Vec<String>,
    /// User-defined function names.
    pub functions: Vec<String>,
    /// Per-table column lists for `table.column` dot-notation completion.
    ///
    /// Key: plain table name; value: list of column names for that table.
    pub table_columns: Vec<(String, Vec<String>)>,
}

impl SchemaCache {
    /// Create an empty shared cache.
    pub fn new_shared() -> SharedSchemaCache {
        Arc::new(RwLock::new(Self::default()))
    }

    /// Refresh the cache by running catalog queries against `client`.
    ///
    /// This is intentionally best-effort: a query failure returns an error
    /// but the old cached data remains intact until the next successful refresh.
    pub async fn refresh(cache: &SharedSchemaCache, client: &Client) -> Result<()> {
        let map_err = |e: tokio_postgres::Error| crate::error::PgCliError::Query(e.to_string());

        let tables = client
            .query(
                "SELECT n.nspname, c.relname \
                 FROM pg_catalog.pg_class c \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE c.relkind IN ('r','v','m','f','p') \
                 AND n.nspname NOT IN ('pg_catalog','information_schema') \
                 AND n.nspname NOT LIKE 'pg_temp_%' \
                 AND n.nspname NOT LIKE 'pg_toast%' \
                 ORDER BY 1,2 LIMIT 2000",
                &[],
            )
            .await
            .map_err(map_err)?;

        let cols = client
            .query(
                "SELECT DISTINCT a.attname \
                 FROM pg_catalog.pg_attribute a \
                 JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE a.attnum > 0 AND NOT a.attisdropped \
                 AND c.relkind IN ('r','v','m') \
                 AND n.nspname NOT IN ('pg_catalog','information_schema') \
                 ORDER BY 1 LIMIT 5000",
                &[],
            )
            .await
            .map_err(|e| crate::error::PgCliError::Query(e.to_string()))?;

        // Per-table column mapping for dot-notation completion.
        let table_col_rows = client
            .query(
                "SELECT c.relname, a.attname \
                 FROM pg_catalog.pg_attribute a \
                 JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
                 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                 WHERE a.attnum > 0 AND NOT a.attisdropped \
                 AND c.relkind IN ('r','v','m') \
                 AND n.nspname NOT IN ('pg_catalog','information_schema') \
                 ORDER BY c.relname, a.attnum LIMIT 10000",
                &[],
            )
            .await
            .map_err(|e| crate::error::PgCliError::Query(e.to_string()))?;

        let schemas = client
            .query(
                "SELECT nspname FROM pg_catalog.pg_namespace \
                 WHERE nspname NOT LIKE 'pg_temp_%' \
                 AND nspname NOT LIKE 'pg_toast%' \
                 ORDER BY 1",
                &[],
            )
            .await
            .map_err(|e| crate::error::PgCliError::Query(e.to_string()))?;

        let funcs = client
            .query(
                "SELECT DISTINCT p.proname FROM pg_catalog.pg_proc p \
                 JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
                 WHERE n.nspname NOT IN ('pg_catalog','information_schema') \
                 ORDER BY 1 LIMIT 500",
                &[],
            )
            .await
            .map_err(|e| crate::error::PgCliError::Query(e.to_string()))?;

        let mut w = cache.write().unwrap();

        w.table_names.clear();
        w.qualified_tables.clear();
        for row in &tables {
            let schema: &str = row.get(0);
            let name: &str = row.get(1);
            w.qualified_tables.push(format!("{schema}.{name}"));
            let name_owned = name.to_string();
            if !w.table_names.contains(&name_owned) {
                w.table_names.push(name_owned);
            }
        }

        w.columns.clear();
        for row in &cols {
            w.columns.push(row.get::<_, &str>(0).to_string());
        }

        w.table_columns.clear();
        let mut cur_table = String::new();
        for row in &table_col_rows {
            let tbl: &str = row.get(0);
            let col: &str = row.get(1);
            if tbl != cur_table {
                cur_table = tbl.to_string();
                w.table_columns.push((cur_table.clone(), Vec::new()));
            }
            if let Some(last) = w.table_columns.last_mut() {
                last.1.push(col.to_string());
            }
        }

        w.schemas.clear();
        for row in &schemas {
            w.schemas.push(row.get::<_, &str>(0).to_string());
        }

        w.functions.clear();
        for row in &funcs {
            w.functions.push(row.get::<_, &str>(0).to_string());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_shared_starts_empty() {
        let cache = SchemaCache::new_shared();
        let r = cache.read().unwrap();
        assert!(r.table_names.is_empty());
        assert!(r.schemas.is_empty());
    }

    #[test]
    fn write_then_read() {
        let cache = SchemaCache::new_shared();
        {
            let mut w = cache.write().unwrap();
            w.table_names.push("users".to_string());
            w.schemas.push("public".to_string());
        }
        let r = cache.read().unwrap();
        assert_eq!(r.table_names, vec!["users"]);
        assert_eq!(r.schemas, vec!["public"]);
    }

    #[test]
    fn default_cache_all_fields_empty() {
        let cache = SchemaCache::default();
        assert!(cache.table_names.is_empty());
        assert!(cache.qualified_tables.is_empty());
        assert!(cache.schemas.is_empty());
        assert!(cache.columns.is_empty());
        assert!(cache.functions.is_empty());
        assert!(cache.table_columns.is_empty());
    }

    #[test]
    fn qualified_tables_and_columns_round_trip() {
        let cache = SchemaCache::new_shared();
        {
            let mut w = cache.write().unwrap();
            w.qualified_tables.push("public.users".to_string());
            w.columns.push("id".to_string());
            w.columns.push("name".to_string());
            w.functions.push("now".to_string());
            w.table_columns.push((
                "users".to_string(),
                vec!["id".to_string(), "name".to_string()],
            ));
        }
        let r = cache.read().unwrap();
        assert_eq!(r.qualified_tables, vec!["public.users"]);
        assert_eq!(r.columns, vec!["id", "name"]);
        assert_eq!(r.functions, vec!["now"]);
        assert_eq!(r.table_columns.len(), 1);
        assert_eq!(r.table_columns[0].0, "users");
        assert_eq!(r.table_columns[0].1, vec!["id", "name"]);
    }

    #[test]
    fn clearing_cache_fields_resets_to_empty() {
        let cache = SchemaCache::new_shared();
        {
            let mut w = cache.write().unwrap();
            w.table_names.push("a".to_string());
            w.table_names.clear();
        }
        let r = cache.read().unwrap();
        assert!(r.table_names.is_empty());
    }

    #[test]
    fn shared_cache_is_cloneable_and_shares_state() {
        let cache = SchemaCache::new_shared();
        let cache2 = cache.clone();
        {
            let mut w = cache.write().unwrap();
            w.schemas.push("public".to_string());
        }
        let r = cache2.read().unwrap();
        assert_eq!(r.schemas, vec!["public"]);
    }
}
