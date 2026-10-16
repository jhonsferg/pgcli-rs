//! Integration tests against a live PostgreSQL server.
//!
//! They only build with the `integration-tests` feature and only run when
//! `PGCLI_RS_TEST_DSN` points at a server the tests may write to, for
//! example:
//!
//! ```sh
//! PGCLI_RS_TEST_DSN="postgresql://user:pass@localhost:5432/pgcli_test" \
//!   cargo test -p pgcli-cli --features integration-tests --test integration_tests
//! ```
//!
//! Every test creates its own uniquely named objects and drops them, so the
//! tests can run in parallel and against a shared database. Without the
//! variable each test returns early, so a plain `cargo test` never fails for
//! lack of a server.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use pgcli_connection::{config::ConnectionConfig, pool::ConnectionPool};
use pgcli_executor::{query::QueryExecutor, runner::ScriptRunner};
use pgcli_meta::introspection::Introspector;
use pgcli_repl::schema_cache::SchemaCache;

/// The DSN under test, or `None` to skip.
fn dsn() -> Option<String> {
    std::env::var("PGCLI_RS_TEST_DSN")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Returns early from the test when no DSN is configured.
macro_rules! require_dsn {
    () => {
        match dsn() {
            Some(d) => d,
            None => {
                eprintln!("PGCLI_RS_TEST_DSN not set; skipping");
                return;
            }
        }
    };
}

/// A name that no other test (or test run) uses.
fn unique(prefix: &str) -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!(
        "{prefix}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

async fn connect(dsn: &str) -> ConnectionPool {
    let cfg = ConnectionConfig::from_uri(dsn).expect("valid DSN");
    ConnectionPool::connect(&cfg).await.expect("connect")
}

/// Runs the real `pgcli` binary against the DSN with `args`.
fn pgcli(dsn: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pgcli"))
        .arg(dsn)
        .args(args)
        .args(["--no-pager"])
        .env_remove("PGPASSWORD")
        .env("NO_COLOR", "1")
        .output()
        .expect("run pgcli")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A boolean cell as pgcli renders it.
fn is_true(cell: &str) -> bool {
    matches!(cell, "t" | "true")
}

// -- Library --------------------------------------------------------------------

#[tokio::test]
async fn select_one_returns_a_single_row() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let result = QueryExecutor::execute(pool.client(), "SELECT 1 AS one")
        .await
        .expect("query");
    assert_eq!(result.columns.len(), 1);
    assert_eq!(result.columns[0].name, "one");
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].values[0].to_string(), "1");
}

#[tokio::test]
async fn ddl_dml_and_returning_round_trip() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let table = unique("pgcli_it_dml");
    let client = pool.client();

    QueryExecutor::execute(
        client,
        &format!("CREATE TABLE {table} (id serial PRIMARY KEY, name text NOT NULL)"),
    )
    .await
    .expect("create");
    let inserted = QueryExecutor::execute(
        client,
        &format!("INSERT INTO {table} (name) VALUES ('a'), ('b') RETURNING id, name"),
    )
    .await
    .expect("insert");
    assert_eq!(inserted.rows.len(), 2, "RETURNING yields the new rows");

    let updated = QueryExecutor::execute(
        client,
        &format!("UPDATE {table} SET name = 'z' WHERE name = 'a'"),
    )
    .await
    .expect("update");
    assert_eq!(updated.affected_rows, Some(1));

    let rows = QueryExecutor::execute(client, &format!("SELECT name FROM {table} ORDER BY name"))
        .await
        .expect("select");
    let names: Vec<String> = rows.rows.iter().map(|r| r.values[0].to_string()).collect();
    assert_eq!(names, ["b", "z"]);

    QueryExecutor::execute(client, &format!("DROP TABLE {table}"))
        .await
        .expect("drop");
}

#[tokio::test]
async fn empty_result_keeps_the_column_headers() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let result = QueryExecutor::execute(pool.client(), "SELECT 1 AS a, 'x' AS b WHERE false")
        .await
        .expect("query");
    assert!(result.rows.is_empty());
    let names: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[tokio::test]
async fn batch_runs_every_statement_in_order() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let results = QueryExecutor::execute_batch(pool.client(), "SELECT 1; SELECT 2; SELECT 3;")
        .await
        .expect("batch");
    let values: Vec<String> = results
        .iter()
        .map(|r| r.rows[0].values[0].to_string())
        .collect();
    assert_eq!(values, ["1", "2", "3"]);
}

#[tokio::test]
async fn query_errors_carry_the_server_message() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let err = QueryExecutor::execute(pool.client(), "SELECT * FROM pgcli_it_does_not_exist")
        .await
        .expect_err("missing table");
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[tokio::test]
async fn schema_cache_sees_a_new_table() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let table = unique("pgcli_it_cache");
    QueryExecutor::execute(pool.client(), &format!("CREATE TABLE {table} (col_x int)"))
        .await
        .expect("create");

    let cache = SchemaCache::new_shared();
    SchemaCache::refresh(&cache, pool.client())
        .await
        .expect("refresh");
    {
        let c = cache.read().expect("lock");
        assert!(c.table_names.contains(&table), "table is cached");
        assert!(
            c.columns.iter().any(|col| col == "col_x"),
            "column is cached"
        );
        assert!(c.schemas.iter().any(|s| s == "public"), "schema is cached");
    }

    QueryExecutor::execute(pool.client(), &format!("DROP TABLE {table}"))
        .await
        .expect("drop");
}

#[tokio::test]
async fn introspection_describes_a_table() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let table = unique("pgcli_it_desc");
    QueryExecutor::execute(
        pool.client(),
        &format!("CREATE TABLE {table} (id int PRIMARY KEY, label text)"),
    )
    .await
    .expect("create");

    let desc = Introspector::new(pool.client())
        .describe_table("public", &table)
        .await
        .expect("describe");
    assert_eq!(desc.columns.len(), 2);

    QueryExecutor::execute(pool.client(), &format!("DROP TABLE {table}"))
        .await
        .expect("drop");
}

#[tokio::test]
async fn script_runner_substitutes_variables() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("script.sql");
    std::fs::write(&script, "SELECT :'greeting' AS g;\n").expect("write script");

    let mut runner = ScriptRunner::new();
    runner.set_variable("greeting", "hello");
    let results = runner
        .run_file(pool.client(), &script, false)
        .await
        .expect("run script");
    assert_eq!(
        results.last().expect("a result").rows[0].values[0].to_string(),
        "hello"
    );
}

#[tokio::test]
async fn prefer_mode_encrypts_when_the_server_supports_tls() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let ssl_on = QueryExecutor::execute(pool.client(), "SHOW ssl")
        .await
        .expect("show ssl")
        .rows[0]
        .values[0]
        .to_string()
        == "on";
    if !ssl_on {
        eprintln!("server has ssl=off; nothing to check");
        return;
    }
    let encrypted = QueryExecutor::execute(
        pool.client(),
        "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
    )
    .await
    .expect("pg_stat_ssl")
    .rows[0]
        .values[0]
        .to_string();
    assert!(
        is_true(&encrypted),
        "sslmode=prefer must use TLS when the server offers it, as libpq does"
    );
}

#[tokio::test]
async fn show_returns_the_setting_value() {
    let dsn = require_dsn!();
    let pool = connect(&dsn).await;
    let result = QueryExecutor::execute(pool.client(), "SHOW server_version_num")
        .await
        .expect("show");
    assert_eq!(result.rows.len(), 1, "SHOW must return its row");
    let version: u32 = result.rows[0].values[0]
        .to_string()
        .parse()
        .expect("numeric version");
    assert!(version >= 100_000, "{version}");
}

#[tokio::test]
async fn require_mode_never_falls_back_to_plain_text() {
    let dsn = require_dsn!();
    let ssl_on = {
        let pool = connect(&dsn).await;
        let result = QueryExecutor::execute(pool.client(), "SHOW ssl")
            .await
            .expect("show ssl");
        result.rows[0].values[0].to_string() == "on"
    };

    let mut cfg = ConnectionConfig::from_uri(&dsn).expect("valid DSN");
    cfg.tls_mode = pgcli_connection::tls::TlsMode::Require;
    match ConnectionPool::connect(&cfg).await {
        Ok(pool) => {
            assert!(ssl_on, "require must not connect to a server without TLS");
            let encrypted = QueryExecutor::execute(
                pool.client(),
                "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            )
            .await
            .expect("pg_stat_ssl")
            .rows[0]
                .values[0]
                .to_string();
            assert!(is_true(&encrypted), "require must encrypt");
        }
        Err(e) => assert!(!ssl_on, "require failed against a TLS server: {e}"),
    }
}

#[tokio::test]
async fn wrong_password_fails_fast_without_retries() {
    let dsn = require_dsn!();
    let mut cfg = ConnectionConfig::from_uri(&dsn).expect("valid DSN");
    cfg.password = Some("definitely-not-the-password".to_string());

    let started = Instant::now();
    let err = match ConnectionPool::connect(&cfg).await {
        Ok(_) => panic!("a wrong password must not connect"),
        Err(e) => e,
    };
    let elapsed = started.elapsed();
    assert!(err.to_string().contains("password"), "{err}");
    // Retrying an authentication failure only adds back-off delays; the error
    // has to come back after a single attempt.
    assert!(
        elapsed < Duration::from_millis(1000),
        "took {elapsed:?}; authentication errors must not be retried"
    );
}

// -- Binary ---------------------------------------------------------------------

#[test]
fn command_mode_prints_the_result() {
    let dsn = require_dsn!();
    let out = pgcli(&dsn, &["-c", "SELECT 41 + 1 AS answer"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains("answer"), "{text}");
    assert!(text.contains("42"), "{text}");
}

#[test]
fn command_mode_show_prints_the_value() {
    let dsn = require_dsn!();
    let out = pgcli(&dsn, &["--format", "csv", "-c", "SHOW client_encoding"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(text.contains("UTF8"), "{text}");
}

#[test]
fn command_mode_csv_format() {
    let dsn = require_dsn!();
    let out = pgcli(&dsn, &["--format", "csv", "-c", "SELECT 1 AS a, 'x' AS b"]);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.first().copied(), Some("a,b"), "{text}");
    assert_eq!(lines.get(1).copied(), Some("1,x"), "{text}");
}

#[test]
fn command_mode_meta_command_lists_tables() {
    let dsn = require_dsn!();
    let table = unique("pgcli_it_meta");
    let out = pgcli(&dsn, &["-c", &format!("CREATE TABLE {table} (id int)")]);
    assert!(out.status.success(), "{out:?}");

    let out = pgcli(&dsn, &["-c", "\\dt"]);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains(&table), "{}", stdout(&out));

    let out = pgcli(&dsn, &["-c", &format!("DROP TABLE {table}")]);
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn command_mode_reports_errors_with_exit_code_1() {
    let dsn = require_dsn!();
    let out = pgcli(&dsn, &["-c", "SELECT * FROM pgcli_it_does_not_exist"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("does not exist"),
        "{out:?}"
    );
}

#[test]
fn file_mode_runs_a_script() {
    let dsn = require_dsn!();
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("s.sql");
    std::fs::write(&script, "SELECT 'from-file' AS src;\n").expect("write");
    let out = pgcli(&dsn, &["-f", script.to_str().expect("utf-8 path")]);
    assert!(out.status.success(), "{out:?}");
    assert!(stdout(&out).contains("from-file"), "{}", stdout(&out));
}

#[test]
fn export_writes_a_csv_file() {
    let dsn = require_dsn!();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("out.csv");
    let out = pgcli(
        &dsn,
        &[
            "-c",
            "SELECT 7 AS n",
            "--export",
            path.to_str().expect("utf-8 path"),
        ],
    );
    assert!(out.status.success(), "{out:?}");
    let csv = std::fs::read_to_string(&path).expect("exported file");
    assert!(csv.starts_with("n"), "{csv}");
    assert!(csv.contains('7'), "{csv}");
}

#[test]
fn export_runs_the_statement_only_once() {
    let dsn = require_dsn!();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("inserted.csv");
    let table = unique("pgcli_it_export_once");

    let out = pgcli(&dsn, &["-c", &format!("CREATE TABLE {table} (id int)")]);
    assert!(out.status.success(), "{out:?}");
    let out = pgcli(
        &dsn,
        &[
            "-c",
            &format!("INSERT INTO {table} VALUES (1) RETURNING id"),
            "--export",
            path.to_str().expect("utf-8 path"),
        ],
    );
    assert!(out.status.success(), "{out:?}");

    let out = pgcli(
        &dsn,
        &[
            "--format",
            "csv",
            "-c",
            &format!("SELECT count(*) AS n FROM {table}"),
        ],
    );
    let text = stdout(&out);
    assert!(
        text.lines().any(|l| l.trim() == "1"),
        "--export must not run the INSERT twice: {text}"
    );
    let out = pgcli(&dsn, &["-c", &format!("DROP TABLE {table}")]);
    assert!(out.status.success(), "{out:?}");
}

#[test]
fn client_copy_round_trips_through_a_file() {
    let dsn = require_dsn!();
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("copy.csv");
    let file_str = file.to_str().expect("utf-8 path").replace('\\', "/");
    let src = unique("pgcli_it_copy_src");
    let dst = unique("pgcli_it_copy_dst");

    let run = |sql: &str| {
        let out = pgcli(&dsn, &["-c", sql]);
        assert!(out.status.success(), "{sql}: {out:?}");
    };
    run(&format!(
        "CREATE TABLE {src} AS SELECT g AS id FROM generate_series(1, 5) g"
    ));
    run(&format!("CREATE TABLE {dst} (id int)"));

    // `\copy` is handled by the interactive session, so feed it on stdin.
    let session = format!(
        "\\copy {src} TO '{file_str}' WITH (FORMAT csv)\n\\copy {dst} FROM '{file_str}' WITH (FORMAT csv)\n\\q\n"
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_pgcli"))
        .arg(&dsn)
        .arg("--no-pager")
        .env_remove("PGPASSWORD")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn pgcli");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(session.as_bytes())
            .expect("write session");
    }
    let out = child.wait_with_output().expect("pgcli exits");
    assert!(out.status.success(), "{out:?}");
    assert!(Path::new(&file).is_file(), "{out:?}");

    let out = pgcli(
        &dsn,
        &[
            "--format",
            "csv",
            "-c",
            &format!("SELECT count(*) FROM {dst}"),
        ],
    );
    assert!(stdout(&out).contains('5'), "{}", stdout(&out));

    run(&format!("DROP TABLE {src}, {dst}"));
}
