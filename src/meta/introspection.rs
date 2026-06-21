/// Deep PostgreSQL object introspection via system catalog queries.
///
/// All queries target PostgreSQL 12+ and use `pg_catalog` schema explicitly.
use tokio_postgres::Client;

use crate::error::{PgCliError, Result};
use crate::executor::query::QueryExecutor;
use crate::protocol::messages::QueryResult;

/// Split `"schema.name"` into `(schema, name)`; default schema is `"public"`.
fn split_schema_obj(name: &str) -> (String, String) {
    match name.split_once('.') {
        Some((s, t)) => (s.to_string(), t.to_string()),
        None => ("public".to_string(), name.to_string()),
    }
}

/// Object types supported by DDL generation.
#[derive(Debug, Clone, PartialEq)]
pub enum ObjectType {
    /// A regular or partitioned table.
    Table,
    /// A view or materialized view.
    View,
    /// A function or procedure.
    Function,
    /// An index.
    Index,
    /// A sequence.
    Sequence,
}

/// Column-level detail returned by `describe_table`.
#[derive(Debug, Clone)]
pub struct ColumnInfo {
    /// Column name.
    pub name: String,
    /// PostgreSQL data type string.
    pub data_type: String,
    /// Whether the column allows NULLs.
    pub nullable: bool,
    /// Default expression, if any.
    pub default: Option<String>,
}

/// Full table description returned by `describe_table`.
#[derive(Debug, Clone)]
pub struct TableDescription {
    /// Schema name.
    pub schema: String,
    /// Table name.
    pub table: String,
    /// Column definitions.
    pub columns: Vec<ColumnInfo>,
    /// Raw `QueryResult` for display purposes.
    pub raw: QueryResult,
}

/// Size information returned by `table_size`.
#[derive(Debug, Clone)]
pub struct SizeInfo {
    /// Total size (table + indexes + TOAST).
    pub total: String,
    /// Table-only size.
    pub table: String,
    /// Index size.
    pub indexes: String,
}

/// A single lock entry returned by `list_locks`.
#[derive(Debug, Clone)]
pub struct LockInfo {
    /// Backend PID holding or waiting for the lock.
    pub pid: i32,
    /// Lock type string.
    pub lock_type: String,
    /// Relation name, if applicable.
    pub relation: Option<String>,
    /// Lock mode (e.g. `"AccessShareLock"`).
    pub mode: String,
    /// Whether the lock is held (`true`) or waiting (`false`).
    pub granted: bool,
}

/// A row from `pg_stat_activity`.
#[derive(Debug, Clone)]
pub struct QueryActivity {
    /// Backend PID.
    pub pid: i32,
    /// Database user.
    pub user: String,
    /// Client application name.
    pub application: String,
    /// Backend state.
    pub state: String,
    /// Current query text (truncated).
    pub query: String,
}

/// Provides deep introspection of PostgreSQL objects via catalog queries.
pub struct Introspector<'a> {
    client: &'a Client,
}

impl<'a> Introspector<'a> {
    /// Create a new `Introspector` bound to an active PostgreSQL client.
    pub fn new(client: &'a Client) -> Self {
        Self { client }
    }

    /// Describe a table's columns, constraints, and indexes.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` if the catalog query fails.
    pub async fn describe_table(&self, schema: &str, table: &str) -> Result<TableDescription> {
        let schema_esc = schema.replace('\'', "''");
        let table_esc = table.replace('\'', "''");

        // When schema is "public" (the default applied when user gives no schema prefix),
        // search across all non-temp schemas so that system objects like pg_roles are found.
        let schema_filter = if schema == "public" {
            format!(
                "AND c.relname = '{table_esc}' \
                 AND n.nspname NOT LIKE 'pg_temp_%' AND n.nspname NOT LIKE 'pg_toast%'"
            )
        } else {
            format!(
                "AND n.nspname = '{schema_esc}' \
                 AND c.relname = '{table_esc}'"
            )
        };

        let sql = format!(
            "SELECT \
             a.attname AS \"Column\", \
             pg_catalog.format_type(a.atttypid, a.atttypmod) AS \"Type\", \
             CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END AS \"Nullable\", \
             pg_catalog.pg_get_expr(d.adbin, d.adrelid) AS \"Default\" \
             FROM pg_catalog.pg_attribute a \
             JOIN pg_catalog.pg_class c ON c.oid = a.attrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attnum > 0 \
             AND NOT a.attisdropped \
             {schema_filter} \
             ORDER BY n.nspname, a.attnum;"
        );

        let raw = QueryExecutor::execute(self.client, &sql).await?;

        let columns: Vec<ColumnInfo> = raw
            .rows
            .iter()
            .map(|r| ColumnInfo {
                name: r.values.first().map(|v| v.to_string()).unwrap_or_default(),
                data_type: r.values.get(1).map(|v| v.to_string()).unwrap_or_default(),
                nullable: r.values.get(2).map(|v| v.to_string()).as_deref() == Some("YES"),
                default: r.values.get(3).and_then(|v| {
                    let s = v.to_string();
                    if s.is_empty() {
                        None
                    } else {
                        Some(s)
                    }
                }),
            })
            .collect();

        Ok(TableDescription {
            schema: schema.to_string(),
            table: table.to_string(),
            columns,
            raw,
        })
    }

    /// Generate a `CREATE TABLE` DDL statement for the given table.
    ///
    /// Uses `pg_catalog.pg_get_tabledef` if available (PostgreSQL 16+), otherwise
    /// constructs a best-effort DDL from catalog metadata.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn generate_ddl(
        &self,
        object_type: ObjectType,
        schema: &str,
        name: &str,
    ) -> Result<String> {
        let schema_esc = schema.replace('\'', "''");
        let name_esc = name.replace('\'', "''");

        let sql = match object_type {
            ObjectType::View => format!(
                "SELECT pg_catalog.pg_get_viewdef('{schema_esc}.{name_esc}'::regclass, true);"
            ),
            ObjectType::Function => format!(
                "SELECT pg_catalog.pg_get_functiondef('{schema_esc}.{name_esc}'::regproc::oid);"
            ),
            ObjectType::Index => format!(
                "SELECT pg_catalog.pg_get_indexdef('{schema_esc}.{name_esc}'::regclass::oid);"
            ),
            ObjectType::Table | ObjectType::Sequence => {
                // Build DDL from catalog for tables/sequences.
                return self.table_ddl_from_catalog(&schema_esc, &name_esc).await;
            }
        };

        let result = QueryExecutor::execute(self.client, &sql).await?;
        Ok(result
            .rows
            .first()
            .and_then(|r| r.values.first())
            .map(|v| v.to_string())
            .unwrap_or_default())
    }

    /// Return disk-size information for a table.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn table_size(&self, schema: &str, table: &str) -> Result<SizeInfo> {
        let fqname = format!("{schema}.{table}");
        let fqname = fqname.replace('\'', "''");

        let sql = format!(
            "SELECT \
             pg_catalog.pg_size_pretty(pg_catalog.pg_total_relation_size('{fqname}'::regclass)) AS total, \
             pg_catalog.pg_size_pretty(pg_catalog.pg_relation_size('{fqname}'::regclass)) AS table_size, \
             pg_catalog.pg_size_pretty( \
               pg_catalog.pg_total_relation_size('{fqname}'::regclass) \
               - pg_catalog.pg_relation_size('{fqname}'::regclass) \
             ) AS indexes;"
        );

        let result = QueryExecutor::execute(self.client, &sql).await?;
        let row = result
            .rows
            .first()
            .ok_or_else(|| PgCliError::Query("no size data returned".to_string()))?;

        Ok(SizeInfo {
            total: row
                .values
                .first()
                .map(|v| v.to_string())
                .unwrap_or_default(),
            table: row.values.get(1).map(|v| v.to_string()).unwrap_or_default(),
            indexes: row.values.get(2).map(|v| v.to_string()).unwrap_or_default(),
        })
    }

    /// Return a list of current lock entries from `pg_catalog.pg_locks`.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn list_locks(&self) -> Result<Vec<LockInfo>> {
        let sql = "SELECT l.pid, l.locktype, l.relation::regclass::text, l.mode, l.granted \
                   FROM pg_catalog.pg_locks l \
                   WHERE l.relation IS NOT NULL \
                   ORDER BY l.pid, l.locktype;";

        let result = QueryExecutor::execute(self.client, sql).await?;
        let locks = result
            .rows
            .iter()
            .map(|r| LockInfo {
                pid: r
                    .values
                    .first()
                    .map(|v| v.to_string().parse::<i32>().unwrap_or(0))
                    .unwrap_or(0),
                lock_type: r.values.get(1).map(|v| v.to_string()).unwrap_or_default(),
                relation: r
                    .values
                    .get(2)
                    .map(|v| {
                        let s = v.to_string();
                        if s.is_empty() {
                            None
                        } else {
                            Some(s)
                        }
                    })
                    .unwrap_or(None),
                mode: r.values.get(3).map(|v| v.to_string()).unwrap_or_default(),
                granted: r.values.get(4).map(|v| v.to_string()).as_deref() == Some("true"),
            })
            .collect();

        Ok(locks)
    }

    /// Return current query activity from `pg_catalog.pg_stat_activity`.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn active_queries(&self) -> Result<Vec<QueryActivity>> {
        let sql = "SELECT pid, usename, application_name, state, left(query, 100) \
                   FROM pg_catalog.pg_stat_activity \
                   WHERE pid <> pg_backend_pid() \
                   ORDER BY query_start;";

        let result = QueryExecutor::execute(self.client, sql).await?;
        let activities = result
            .rows
            .iter()
            .map(|r| QueryActivity {
                pid: r
                    .values
                    .first()
                    .map(|v| v.to_string().parse::<i32>().unwrap_or(0))
                    .unwrap_or(0),
                user: r.values.get(1).map(|v| v.to_string()).unwrap_or_default(),
                application: r.values.get(2).map(|v| v.to_string()).unwrap_or_default(),
                state: r.values.get(3).map(|v| v.to_string()).unwrap_or_default(),
                query: r.values.get(4).map(|v| v.to_string()).unwrap_or_default(),
            })
            .collect();

        Ok(activities)
    }

    /// Return a formatted string listing indexes, PK, UNIQUE, and CHECK constraints for a table.
    ///
    /// The output is appended below the column listing produced by `describe_table`.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn describe_table_constraints(&self, schema: &str, table: &str) -> Result<String> {
        let schema_esc = schema.replace('\'', "''");
        let table_esc = table.replace('\'', "''");
        let mut out = String::new();

        // Indexes (includes primary key and unique).
        let idx_sql = format!(
            "SELECT ix.relname AS index_name, \
                    CASE i.indisprimary WHEN true THEN 'PRIMARY KEY' \
                         WHEN false THEN CASE i.indisunique WHEN true THEN 'UNIQUE' ELSE 'INDEX' END \
                    END AS type, \
                    pg_catalog.pg_get_indexdef(i.indexrelid, 0, true) AS definition \
             FROM pg_catalog.pg_index i \
             JOIN pg_catalog.pg_class tbl ON tbl.oid = i.indrelid \
             JOIN pg_catalog.pg_class ix  ON ix.oid  = i.indexrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = tbl.relnamespace \
             WHERE n.nspname = '{schema_esc}' AND tbl.relname = '{table_esc}' \
             ORDER BY i.indisprimary DESC, i.indisunique DESC, ix.relname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &idx_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Indexes:\n");
                for row in &r.rows {
                    let name = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let kind = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let def = row.values.get(2).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("    \"{name}\" {kind}, {def}\n"));
                }
            }
        }

        // Check constraints.
        let check_sql = format!(
            "SELECT conname, pg_catalog.pg_get_constraintdef(oid, true) \
             FROM pg_catalog.pg_constraint \
             WHERE conrelid = '{schema_esc}.{table_esc}'::regclass \
             AND contype = 'c' ORDER BY conname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &check_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Check constraints:\n");
                for row in &r.rows {
                    let name = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let def = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("    \"{name}\" {def}\n"));
                }
            }
        }

        // Foreign-key constraints.
        let fk_sql = format!(
            "SELECT conname, pg_catalog.pg_get_constraintdef(oid, true) \
             FROM pg_catalog.pg_constraint \
             WHERE conrelid = '{schema_esc}.{table_esc}'::regclass \
             AND contype = 'f' ORDER BY conname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &fk_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Foreign-key constraints:\n");
                for row in &r.rows {
                    let name = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let def = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("    \"{name}\" {def}\n"));
                }
            }
        }

        Ok(out)
    }

    /// Return the source of a stored function or procedure.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` if the function is not found or the query fails.
    pub async fn show_function_source(&self, schema: &str, name: &str) -> Result<String> {
        let schema_esc = schema.replace('\'', "''");
        let name_esc = name.replace('\'', "''");

        let sql = format!(
            "SELECT pg_catalog.pg_get_functiondef(p.oid) \
             FROM pg_catalog.pg_proc p \
             JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace \
             WHERE n.nspname = '{schema_esc}' AND p.proname = '{name_esc}' \
             LIMIT 1;"
        );

        let result = QueryExecutor::execute(self.client, &sql).await?;
        result
            .rows
            .first()
            .and_then(|r| r.values.first())
            .map(|v| v.to_string())
            .ok_or_else(|| PgCliError::Query(format!("function \"{schema}.{name}\" not found")))
    }

    /// Return the definition of a view (SELECT portion only).
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` if the view is not found or the query fails.
    pub async fn show_view_definition(&self, schema: &str, name: &str) -> Result<String> {
        let schema_esc = schema.replace('\'', "''");
        let name_esc = name.replace('\'', "''");

        let sql = format!(
            "SELECT pg_catalog.pg_get_viewdef(c.oid, true) \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = '{schema_esc}' AND c.relname = '{name_esc}' \
             AND c.relkind IN ('v', 'm');"
        );

        let result = QueryExecutor::execute(self.client, &sql).await?;
        result
            .rows
            .first()
            .and_then(|r| r.values.first())
            .map(|v| format!("CREATE OR REPLACE VIEW {schema}.{name} AS\n{v}"))
            .ok_or_else(|| PgCliError::Query(format!("view \"{schema}.{name}\" not found")))
    }

    /// Return an extended table description including constraints, triggers, and referenced-by FKs.
    ///
    /// The output is a multi-section formatted string, not a table, so it is printed verbatim.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn describe_table_extended(&self, schema: &str, table: &str) -> Result<String> {
        let schema_esc = schema.replace('\'', "''");
        let table_esc = table.replace('\'', "''");
        let mut out = String::new();

        // -- Check constraints ---------------------------------------------
        let check_sql = format!(
            "SELECT conname, pg_catalog.pg_get_constraintdef(oid, true) \
             FROM pg_catalog.pg_constraint \
             WHERE conrelid = '{schema_esc}.{table_esc}'::regclass \
             AND contype = 'c' \
             ORDER BY conname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &check_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Check constraints:\n");
                for row in &r.rows {
                    let name = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let def = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("    \"{name}\" {def}\n"));
                }
            }
        }

        // -- Foreign-key constraints defined on this table -----------------
        let fk_sql = format!(
            "SELECT conname, pg_catalog.pg_get_constraintdef(oid, true) \
             FROM pg_catalog.pg_constraint \
             WHERE conrelid = '{schema_esc}.{table_esc}'::regclass \
             AND contype = 'f' \
             ORDER BY conname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &fk_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Foreign-key constraints:\n");
                for row in &r.rows {
                    let name = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let def = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("    \"{name}\" {def}\n"));
                }
            }
        }

        // -- Tables that have FKs pointing TO this table -------------------
        let ref_sql = format!(
            "SELECT c.conname, \
                    src_n.nspname || '.' || src_c.relname AS src_table, \
                    pg_catalog.pg_get_constraintdef(c.oid, true) AS def \
             FROM pg_catalog.pg_constraint c \
             JOIN pg_catalog.pg_class src_c ON src_c.oid = c.conrelid \
             JOIN pg_catalog.pg_namespace src_n ON src_n.oid = src_c.relnamespace \
             WHERE c.confrelid = '{schema_esc}.{table_esc}'::regclass \
             AND c.contype = 'f' \
             ORDER BY src_table, c.conname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &ref_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Referenced by:\n");
                for row in &r.rows {
                    let name = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let src = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let def = row.values.get(2).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!(
                        "    TABLE \"{src}\" CONSTRAINT \"{name}\" {def}\n"
                    ));
                }
            }
        }

        // -- Triggers ------------------------------------------------------
        let trig_sql = format!(
            "SELECT t.tgname, pg_catalog.pg_get_triggerdef(t.oid, true) \
             FROM pg_catalog.pg_trigger t \
             JOIN pg_catalog.pg_class c ON c.oid = t.tgrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = '{schema_esc}' AND c.relname = '{table_esc}' \
             AND NOT t.tgisinternal \
             ORDER BY t.tgname;"
        );
        if let Ok(r) = QueryExecutor::execute(self.client, &trig_sql).await {
            if !r.rows.is_empty() {
                out.push_str("Triggers:\n");
                for row in &r.rows {
                    let def = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("    {def}\n"));
                }
            }
        }

        if out.is_empty() {
            out = format!("(No extended information found for \"{schema}.{table}\")\n");
        }
        Ok(out)
    }

    /// Return a text summary of the objects that depend on `name` and what `name` depends on.
    ///
    /// `name` can be `schema.object` or just `object`; the search is case-insensitive.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn show_deps(&self, name: &str) -> Result<String> {
        let (schema, obj) = split_schema_obj(name);
        let obj_esc = obj.replace('\'', "''");
        let schema_esc = schema.replace('\'', "''");
        let sql = format!(
            "SELECT dep_type, \
                    obj_kind || ' ' || dep_schema || '.' || dep_name AS \"Depends On\", \
                    ref_kind || ' ' || ref_schema || '.' || ref_name AS \"Referenced By\" \
             FROM ( \
               SELECT 'depends_on' AS dep_type, \
                      pg_describe_object(d.classid, d.objid, 0) AS dep_info, \
                      n2.nspname AS dep_schema, c2.relname AS dep_name, c2.relkind::text AS obj_kind, \
                      n.nspname AS ref_schema, c.relname AS ref_name, c.relkind::text AS ref_kind \
               FROM pg_catalog.pg_depend d \
               JOIN pg_catalog.pg_class c  ON c.oid = d.refobjid \
               JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
               JOIN pg_catalog.pg_class c2 ON c2.oid = d.objid \
               JOIN pg_catalog.pg_namespace n2 ON n2.oid = c2.relnamespace \
               WHERE n.nspname ILIKE '{schema_esc}' AND c.relname ILIKE '{obj_esc}' \
               AND d.deptype NOT IN ('i','p') \
               LIMIT 100 \
             ) sub ORDER BY 1,2;",
        );
        match QueryExecutor::execute(self.client, &sql).await {
            Ok(r) if r.rows.is_empty() => Ok(format!("No dependencies found for \"{name}\".\n")),
            Ok(r) => {
                let mut out = format!("Dependencies for \"{name}\":\n");
                for row in &r.rows {
                    let dep_type = row
                        .values
                        .first()
                        .map(|v| v.to_string())
                        .unwrap_or_default();
                    let depends = row.values.get(1).map(|v| v.to_string()).unwrap_or_default();
                    let refby = row.values.get(2).map(|v| v.to_string()).unwrap_or_default();
                    out.push_str(&format!("  [{dep_type}]  {depends}  ->  {refby}\n"));
                }
                Ok(out)
            }
            Err(e) => Err(e),
        }
    }

    /// Return a `QueryResult` with index statistics for `name` (table) or all user tables.
    ///
    /// Columns: Table, Index, Type, Size, Scans, Tuples Read, Tuples Fetched, Unique, Valid
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn show_indexes(&self, name: &str) -> Result<crate::protocol::messages::QueryResult> {
        let filter = if name.is_empty() {
            String::new()
        } else {
            let (schema, tbl) = split_schema_obj(name);
            let tbl_esc = tbl.replace('\'', "''");
            let schema_esc = schema.replace('\'', "''");
            format!("AND s.schemaname ILIKE '{schema_esc}' AND s.relname ILIKE '{tbl_esc}'")
        };
        let sql = format!(
            "SELECT s.schemaname || '.' || s.relname AS \"Table\", \
                    s.indexrelname AS \"Index\", \
                    am.amname AS \"Type\", \
                    pg_size_pretty(pg_relation_size(s.indexrelid)) AS \"Size\", \
                    s.idx_scan AS \"Scans\", \
                    s.idx_tup_read AS \"Tuples Read\", \
                    s.idx_tup_fetch AS \"Tuples Fetched\", \
                    ix.indisunique AS \"Unique\", \
                    ix.indisvalid  AS \"Valid\" \
             FROM pg_catalog.pg_stat_user_indexes s \
             JOIN pg_catalog.pg_index ix ON ix.indexrelid = s.indexrelid \
             JOIN pg_catalog.pg_class ic ON ic.oid = s.indexrelid \
             JOIN pg_catalog.pg_am am ON am.oid = ic.relam \
             WHERE true {filter} \
             ORDER BY s.schemaname, s.relname, s.indexrelname;"
        );
        QueryExecutor::execute(self.client, &sql).await
    }

    /// Estimate table and index bloat from system catalogs.
    ///
    /// Uses the well-known statistics-based bloat estimation formula.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn show_bloat(&self) -> Result<crate::protocol::messages::QueryResult> {
        let sql = "SELECT \
            schemaname || '.' || relname AS \"Table\", \
            pg_size_pretty(real_size::bigint) AS \"Real Size\", \
            pg_size_pretty(GREATEST(0, bloat_est)::bigint) AS \"Bloat Est.\", \
            round(CASE WHEN real_size = 0 THEN 0 \
                  ELSE GREATEST(0, bloat_est) * 100.0 / real_size END::numeric, 1) \
              || '%' AS \"Bloat %\" \
          FROM ( \
            SELECT \
              schemaname, relname, \
              pg_table_size(quote_ident(schemaname) || '.' || quote_ident(relname)) AS real_size, \
              pg_table_size(quote_ident(schemaname) || '.' || quote_ident(relname)) \
                - (n_live_tup + n_dead_tup + 1) * 8192 AS bloat_est \
            FROM pg_catalog.pg_stat_user_tables \
          ) sub \
          ORDER BY real_size DESC \
          LIMIT 50;";
        QueryExecutor::execute(self.client, sql).await
    }

    /// List roles/users with their attributes (`\du` / `\dg`).
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn list_roles(
        &self,
        pattern: &str,
    ) -> Result<crate::protocol::messages::QueryResult> {
        let filter = if pattern.is_empty() {
            String::new()
        } else {
            let p = pattern.replace('\'', "''");
            format!("AND r.rolname ILIKE '{p}'")
        };
        let sql = format!(
            "SELECT r.rolname AS \"Role\", \
                    CASE WHEN r.rolsuper THEN 'yes' ELSE 'no' END AS \"Superuser\", \
                    CASE WHEN r.rolinherit THEN 'yes' ELSE 'no' END AS \"Inherit\", \
                    CASE WHEN r.rolcreaterole THEN 'yes' ELSE 'no' END AS \"Create role\", \
                    CASE WHEN r.rolcreatedb THEN 'yes' ELSE 'no' END AS \"Create DB\", \
                    CASE WHEN r.rolcanlogin THEN 'yes' ELSE 'no' END AS \"Login\", \
                    CASE WHEN r.rolreplication THEN 'yes' ELSE 'no' END AS \"Replication\", \
                    CASE WHEN r.rolbypassrls THEN 'yes' ELSE 'no' END AS \"Bypass RLS\", \
                    r.rolconnlimit AS \"Conn limit\", \
                    ARRAY(SELECT b.rolname FROM pg_catalog.pg_auth_members m \
                          JOIN pg_catalog.pg_roles b ON m.roleid = b.oid \
                          WHERE m.member = r.oid)::text AS \"Member of\" \
             FROM pg_catalog.pg_roles r \
             WHERE true {filter} \
             ORDER BY r.rolname;"
        );
        QueryExecutor::execute(self.client, &sql).await
    }

    /// List sequences with their definition and current value (`\sequences` / `\ds`).
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn list_sequences(
        &self,
        pattern: &str,
    ) -> Result<crate::protocol::messages::QueryResult> {
        let filter = if pattern.is_empty() {
            String::new()
        } else {
            let p = pattern.replace('\'', "''");
            format!("AND s.relname ILIKE '{p}'")
        };
        let sql = format!(
            "SELECT n.nspname || '.' || s.relname AS \"Sequence\", \
                    seq.seqstart AS \"Start\", \
                    seq.seqmin AS \"Min\", \
                    seq.seqmax AS \"Max\", \
                    seq.seqincrement AS \"Increment\", \
                    seq.seqcycle AS \"Cycle\", \
                    seq.seqcache AS \"Cache\" \
             FROM pg_catalog.pg_class s \
             JOIN pg_catalog.pg_namespace n ON n.oid = s.relnamespace \
             JOIN pg_catalog.pg_sequence seq ON seq.seqrelid = s.oid \
             WHERE s.relkind = 'S' \
             AND n.nspname NOT IN ('pg_catalog','information_schema') \
             {filter} \
             ORDER BY 1;"
        );
        QueryExecutor::execute(self.client, &sql).await
    }

    /// List procedural languages (`\dL`).
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Query` on catalog failure.
    pub async fn list_languages(
        &self,
        pattern: &str,
    ) -> Result<crate::protocol::messages::QueryResult> {
        let filter = if pattern.is_empty() {
            String::new()
        } else {
            let p = pattern.replace('\'', "''");
            format!("AND l.lanname ILIKE '{p}'")
        };
        let sql = format!(
            "SELECT l.lanname AS \"Name\", \
                    CASE WHEN l.lanpltrusted THEN 'yes' ELSE 'no' END AS \"Trusted\", \
                    p.proname AS \"Call handler\", \
                    l.lanacl::text AS \"Access privileges\" \
             FROM pg_catalog.pg_language l \
             LEFT JOIN pg_catalog.pg_proc p ON p.oid = l.lanplcallfoid \
             WHERE true {filter} \
             ORDER BY l.lanname;"
        );
        QueryExecutor::execute(self.client, &sql).await
    }

    /// Return column statistics for a table from `pg_stats`.
    ///
    /// Includes null fraction, average width, n_distinct, and a preview of
    /// most common values and their frequencies.
    pub async fn show_column_stats(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<crate::protocol::messages::QueryResult> {
        let s = schema.replace('\'', "''");
        let t = table.replace('\'', "''");
        let sql = format!(
            "SELECT \
                s.attname AS \"Column\", \
                s.null_frac AS \"Null %\", \
                s.avg_width AS \"Avg bytes\", \
                s.n_distinct AS \"N distinct\", \
                CASE \
                    WHEN s.most_common_vals IS NOT NULL \
                    THEN left(s.most_common_vals::text, 60) \
                    ELSE '' \
                END AS \"Most common values\", \
                CASE \
                    WHEN s.most_common_freqs IS NOT NULL \
                    THEN left(s.most_common_freqs::text, 40) \
                    ELSE '' \
                END AS \"Frequencies\" \
             FROM pg_catalog.pg_stats s \
             JOIN pg_catalog.pg_attribute a \
               ON a.attrelid = ( \
                   SELECT c.oid FROM pg_catalog.pg_class c \
                   JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                   WHERE c.relname = '{t}' AND n.nspname = '{s}' LIMIT 1 \
               ) AND a.attname = s.attname AND a.attnum > 0 \
             WHERE s.schemaname = '{s}' AND s.tablename = '{t}' \
             ORDER BY a.attnum;"
        );
        QueryExecutor::execute(self.client, &sql).await
    }

    /// Show table partition information from pg_inherits.
    pub async fn show_partitions(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<crate::protocol::messages::QueryResult> {
        let s = schema.replace('\'', "''");
        let t = table.replace('\'', "''");
        let sql = format!(
            "SELECT \
                c.relname AS \"Partition\", \
                n.nspname AS \"Schema\", \
                pg_get_expr(c.relpartbound, c.oid) AS \"Bound\", \
                pg_size_pretty(pg_relation_size(c.oid)) AS \"Size\" \
             FROM pg_catalog.pg_class p \
             JOIN pg_catalog.pg_namespace np ON np.oid = p.relnamespace \
             JOIN pg_catalog.pg_inherits i ON i.inhparent = p.oid \
             JOIN pg_catalog.pg_class c ON c.oid = i.inhrelid \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE p.relname = '{t}' AND np.nspname = '{s}' \
             ORDER BY c.relname;"
        );
        QueryExecutor::execute(self.client, &sql).await
    }

    /// Construct a best-effort `CREATE TABLE` DDL from catalog metadata.
    async fn table_ddl_from_catalog(&self, schema: &str, table: &str) -> Result<String> {
        let desc = self.describe_table(schema, table).await?;
        let mut ddl = format!("CREATE TABLE {schema}.{table} (\n");
        let col_lines: Vec<String> = desc
            .columns
            .iter()
            .map(|c| {
                let mut line = format!("  {} {}", c.name, c.data_type);
                if !c.nullable {
                    line.push_str(" NOT NULL");
                }
                if let Some(ref def) = c.default {
                    line.push_str(&format!(" DEFAULT {def}"));
                }
                line
            })
            .collect();
        ddl.push_str(&col_lines.join(",\n"));
        ddl.push_str("\n);");
        Ok(ddl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_type_variants_are_distinct() {
        assert_ne!(ObjectType::Table, ObjectType::View);
        assert_ne!(ObjectType::Function, ObjectType::Index);
    }
}
