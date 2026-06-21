# Changelog

All notable changes to pgcli-rs will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
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
