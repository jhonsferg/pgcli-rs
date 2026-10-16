/// Live schema-object cache for schema-aware tab completion.
///
/// Populated once after connecting and refreshed on `\c` reconnect.
/// Uses `std::sync::RwLock` so the sync `Completer` trait can read it
/// without blocking the async runtime.
use std::collections::HashSet;
use std::sync::{Arc, PoisonError, RwLock};

use tokio::task::JoinHandle;
use tokio_postgres::{Client, SimpleQueryMessage};

use pgcli_core::error::Result;

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
    /// The five catalog queries are sent together as one simple-protocol
    /// request, so the refresh costs a single round trip to the server.
    ///
    /// This is intentionally best-effort: a query failure returns an error
    /// but the old cached data remains intact until the next successful refresh.
    pub async fn refresh(cache: &SharedSchemaCache, client: &Client) -> Result<()> {
        let messages = client
            .simple_query(CATALOG_QUERIES)
            .await
            .map_err(|e| pgcli_core::error::PgCliError::Query(e.to_string()))?;

        // One section of rows per query, in order; each ends with CommandComplete.
        let mut sections: [Vec<Vec<String>>; 5] = Default::default();
        let mut section = 0;
        for message in &messages {
            match message {
                SimpleQueryMessage::Row(row) if section < sections.len() => {
                    sections[section].push(
                        (0..row.len())
                            .map(|i| row.get(i).unwrap_or_default().to_string())
                            .collect(),
                    );
                }
                SimpleQueryMessage::CommandComplete(_) => section += 1,
                _ => {}
            }
        }
        let [tables, columns, table_columns, schemas, functions] = sections;

        let fresh = Self::from_rows(&tables, &columns, &table_columns, &schemas, &functions);
        *cache.write().unwrap_or_else(PoisonError::into_inner) = fresh;
        Ok(())
    }

    /// Refreshes the cache in a background task, so the caller (the prompt)
    /// does not wait for the catalog queries. A failure is only logged.
    pub fn spawn_refresh(cache: SharedSchemaCache, client: Arc<Client>) -> JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = Self::refresh(&cache, &client).await {
                tracing::warn!("Schema cache refresh failed: {e}");
            }
        })
    }

    /// Builds a cache from the text rows of the catalog queries.
    fn from_rows(
        tables: &[Vec<String>],
        columns: &[Vec<String>],
        table_columns: &[Vec<String>],
        schemas: &[Vec<String>],
        functions: &[Vec<String>],
    ) -> Self {
        let field = |row: &Vec<String>, i: usize| row.get(i).cloned().unwrap_or_default();

        let mut cache = Self::default();
        let mut seen_tables = HashSet::new();
        for row in tables {
            let (schema, name) = (field(row, 0), field(row, 1));
            cache.qualified_tables.push(format!("{schema}.{name}"));
            if seen_tables.insert(name.clone()) {
                cache.table_names.push(name);
            }
        }

        cache.columns = columns.iter().map(|row| field(row, 0)).collect();

        for row in table_columns {
            let (table, column) = (field(row, 0), field(row, 1));
            match cache.table_columns.last_mut() {
                Some((last, cols)) if *last == table => cols.push(column),
                _ => cache.table_columns.push((table, vec![column])),
            }
        }

        cache.schemas = schemas.iter().map(|row| field(row, 0)).collect();
        cache.functions = functions.iter().map(|row| field(row, 0)).collect();
        cache
    }
}

/// Catalog queries behind the cache, in the order [`SchemaCache::refresh`]
/// reads them: tables, distinct columns, per-table columns, schemas and
/// functions.
const CATALOG_QUERIES: &str = "\
SELECT n.nspname, c.relname \
 FROM pg_catalog.pg_class c \
 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
 WHERE c.relkind IN ('r','v','m','f','p') \
 AND n.nspname NOT IN ('pg_catalog','information_schema') \
 AND n.nspname NOT LIKE 'pg_temp_%' \
 AND n.nspname NOT LIKE 'pg_toast%' \
 ORDER BY 1,2 LIMIT 2000; \
SELECT DISTINCT a.attname \
 FROM pg_catalog.pg_attribute a \
 JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
 WHERE a.attnum > 0 AND NOT a.attisdropped \
 AND c.relkind IN ('r','v','m') \
 AND n.nspname NOT IN ('pg_catalog','information_schema') \
 ORDER BY 1 LIMIT 5000; \
SELECT c.relname, a.attname \
 FROM pg_catalog.pg_attribute a \
 JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
 JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
 WHERE a.attnum > 0 AND NOT a.attisdropped \
 AND c.relkind IN ('r','v','m') \
 AND n.nspname NOT IN ('pg_catalog','information_schema') \
 ORDER BY c.relname, a.attnum LIMIT 10000; \
SELECT nspname FROM pg_catalog.pg_namespace \
 WHERE nspname NOT LIKE 'pg_temp_%' \
 AND nspname NOT LIKE 'pg_toast%' \
 ORDER BY 1; \
SELECT DISTINCT p.proname FROM pg_catalog.pg_proc p \
 JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
 WHERE n.nspname NOT IN ('pg_catalog','information_schema') \
 ORDER BY 1 LIMIT 500";

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(data: &[&[&str]]) -> Vec<Vec<String>> {
        data.iter()
            .map(|r| r.iter().map(|s| (*s).to_string()).collect())
            .collect()
    }

    #[test]
    fn from_rows_dedupes_table_names_and_groups_columns() {
        let cache = SchemaCache::from_rows(
            &rows(&[
                &["public", "users"],
                &["audit", "users"],
                &["public", "orders"],
            ]),
            &rows(&[&["id"], &["name"]]),
            &rows(&[&["orders", "id"], &["users", "id"], &["users", "name"]]),
            &rows(&[&["audit"], &["public"]]),
            &rows(&[&["now_utc"]]),
        );
        assert_eq!(cache.table_names, ["users", "orders"]);
        assert_eq!(
            cache.qualified_tables,
            ["public.users", "audit.users", "public.orders"]
        );
        assert_eq!(cache.columns, ["id", "name"]);
        assert_eq!(cache.table_columns.len(), 2);
        assert_eq!(
            cache.table_columns[1],
            (
                "users".to_string(),
                vec!["id".to_string(), "name".to_string()]
            )
        );
        assert_eq!(cache.schemas, ["audit", "public"]);
        assert_eq!(cache.functions, ["now_utc"]);
    }

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
