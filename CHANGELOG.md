# Changelog

All notable changes to pgcli-rs will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `pgcli --upgrade` downloads the latest release, verifies its SHA-256 against the release `checksums.txt` and replaces the running binary in place; `--upgrade --check` only reports whether a newer release exists.
- Install scripts for Linux/macOS (`install/install.sh`) and Windows (`install/install.ps1`): they download the latest release, verify its SHA-256, and place `pgcli` in `~/.local/bin` (`%USERPROFILE%\.local\bin` on Windows) without touching the shell profile, PATH or registry.
- Initial project scaffold: all modules stubbed and documented.
- `PgCliError` unified error type with all required variants.
- `CliArgs` with full `psql` flag compatibility plus pgcli-rs extensions.
- `ConnectionConfig` with URI parsing, env var merging, and `~/.pgpass` support.
- `TlsConfig` with Disabled / Prefer / Require / VerifyFull modes.
- `ConnectionPool` with single-connection and exponential-backoff reconnect.
- `SCRAM-SHA-256` authentication client (RFC 5802).
- `MD5` authentication (inline implementation).
- `QueryResult`, `Column`, `Row`, `CellValue` typed result types.
- `QueryExecutor` for single and batch SQL statement execution.
- `ScriptPipeline` for `.sql` file execution with transaction support.
- `TableFormatter` using `comfy-table` with border levels and expanded mode.
- All standard `psql` output formats: CSV, JSON, JSONL, TSV, HTML, Unaligned.
- `Pager` with terminal height detection and `$PAGER` integration.
- `ReplEditor` wrapping `rustyline` with multi-line detection.
- `HistoryManager` with deduplication and configurable max size.
- `SqlHighlighter` for keyword, string, and comment coloring.
- All standard psql backslash meta-commands.
- pgcli-rs extension meta-commands: `\size`, `\locks`, `\activity`, `\format`.
- `Introspector` for deep catalog introspection.
- `CsvExporter`, `JsonExporter`, `SqlExporter` for data export.
- `ScriptRunner` with `\set` variable substitution and `\if`/`\endif` conditionals.
- Complete unit test suite for all modules.
- GitHub Actions CI, release, and security audit workflows.

### Changed
- The code is split into a Cargo workspace: `pgcli-core`, `pgcli-connection`, `pgcli-executor`, `pgcli-output`, `pgcli-meta`, `pgcli-repl` and `pgcli-cli` (the `pgcli` binary).
- The interactive prompt appears without waiting for the schema cache: it now loads in the background, in a single round trip instead of ten.
- Each statement typed at the prompt saves a round trip: the transaction indicator is only queried inside an explicit transaction block.
- Large results use about 40% less memory, and `--export` no longer runs the query a second time.
- Dependencies updated to their latest versions; `atty` and `rustls-pemfile` are replaced.
- Client keys for the rustls backend may now be PKCS#8, PKCS#1 or SEC1 PEM.

### Fixed
- `SHOW` and `FETCH` print their rows instead of only the command tag.
- Scripts run with `-f` accept `:'name'` and `:"name"` variables, like the interactive session.
- `--export` with an `INSERT ... RETURNING` inserted the rows twice.
- A wrong password or unknown database fails at once instead of after several seconds of retries.
- Shell completions are generated for the `pgcli` binary instead of `pgcli-rs`.

### Security
- `--require-tls` and `verify-full` no longer fall back to an unencrypted connection when the server does not offer TLS.
- With the rustls backend, `prefer` and `require` encrypt connections to servers with a self-signed certificate, as libpq does, instead of falling back to plain text.
- With the native-tls backend, `verify-full` set through `PGSSLMODE` now verifies the server certificate.
