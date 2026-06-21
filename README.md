<p align="center">
  <img src="assets/logo/ferris_and_elephant.png" alt="pgcli-rs" width="420"/>
</p>

<h1 align="center">pgcli-rs</h1>

<p align="center">
  <em>A self-contained PostgreSQL CLI written in pure Rust. No libpq. No system libraries. One binary.</em>
</p>

<p align="center">
  <a href="https://github.com/jhonsferg/pgcli-rs/actions/workflows/ci.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/jhonsferg/pgcli-rs/ci.yml?style=for-the-badge&logo=githubactions&logoColor=white&label=CI" alt="CI"/>
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/License-MIT-yellow?style=for-the-badge" alt="License MIT"/>
  </a>
  <a href="https://github.com/jhonsferg/pgcli-rs/releases">
    <img src="https://img.shields.io/github/v/release/jhonsferg/pgcli-rs?style=for-the-badge&logo=github&logoColor=white&label=Release" alt="Latest Release"/>
  </a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.75%2B-CE422B?style=for-the-badge&logo=rust&logoColor=white" alt="Rust 1.75+"/>
  <img src="https://img.shields.io/badge/Edition-2021-CE422B?style=for-the-badge&logo=rust&logoColor=white" alt="Rust Edition 2021"/>
  <img src="https://img.shields.io/badge/PostgreSQL-14%20%7C%2015%20%7C%2016%20%7C%2017%20%7C%2018-4169E1?style=for-the-badge&logo=postgresql&logoColor=white" alt="PostgreSQL 14-18"/>
  <img src="https://img.shields.io/badge/Tokio-async%20runtime-2E8B57?style=for-the-badge" alt="Tokio"/>
  <img src="https://img.shields.io/badge/tokio--postgres-wire%20protocol-2E8B57?style=for-the-badge" alt="tokio-postgres"/>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-x86__64%20%7C%20ARM64-FCC624?style=for-the-badge&logo=linux&logoColor=black" alt="Linux"/>
  <img src="https://img.shields.io/badge/macOS-x86__64%20%7C%20ARM64-000000?style=for-the-badge&logo=apple&logoColor=white" alt="macOS"/>
  <img src="https://img.shields.io/badge/Windows-x86__64-0078D6?style=for-the-badge&logo=windows&logoColor=white" alt="Windows"/>
  <img src="https://img.shields.io/badge/musl-static%20binary-4CAF50?style=for-the-badge" alt="musl static binary"/>
  <img src="https://img.shields.io/badge/Docker-scratch%20compatible-2496ED?style=for-the-badge&logo=docker&logoColor=white" alt="Docker scratch"/>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/TLS-native--tls%20%7C%20rustls-0F4C81?style=for-the-badge&logo=letsencrypt&logoColor=white" alt="TLS"/>
  <img src="https://img.shields.io/badge/Auth-SCRAM--SHA--256%20%7C%20MD5-B22222?style=for-the-badge" alt="Auth SCRAM-SHA-256"/>
  <img src="https://img.shields.io/badge/unsafe-forbidden-4CAF50?style=for-the-badge" alt="No unsafe code"/>
  <img src="https://img.shields.io/badge/libpq-zero%20deps-4CAF50?style=for-the-badge" alt="Zero libpq dependencies"/>
</p>

---

## 🤔 Why pgcli-rs?

`psql` requires a full PostgreSQL client installation (libpq, system libraries, OS packages).
This creates friction in containers, CI pipelines, minimal Linux distros, macOS without Homebrew,
and Windows. `pgcli-rs` eliminates that friction: **one binary, zero system dependencies,
cross-platform**.

| Feature                         | psql       | pgcli  |
| ------------------------------- | ---------- | ------ |
| Requires libpq                  | ✅ Yes     | ❌ No  |
| Single static binary            | ❌ No      | ✅ Yes |
| Windows native                  | ⚠️ Limited | ✅ Yes |
| SQL syntax highlighting         | ❌ No      | ✅ Yes |
| JSON / CSV / JSONL / TSV export | ❌ No      | ✅ Yes |
| Deep schema introspection       | ⚠️ Limited | ✅ Yes |
| DDL generation                  | ❌ No      | ✅ Yes |
| Benchmark / profiling mode      | ❌ No      | ✅ Yes |
| SCRAM-SHA-256 auth (pure Rust)  | ✅ Yes     | ✅ Yes |
| musl static binary              | ❌ No      | ✅ Yes |

---

## ✨ Key Features

- 🔌 **Zero dependencies** - pure Rust wire protocol, no libpq, no system libs
- 🦀 **Single static binary** - drop it anywhere and run (musl on Linux)
- 🖥️ **Cross-platform** - Linux, macOS, Windows (x86_64 and ARM64)
- 🔐 **Full auth support** - SCRAM-SHA-256, MD5, Trust, `.pgpass` file
- 🔒 **TLS modes** - disable / prefer / require / verify-full
- 🎨 **SQL syntax highlighting** - real-time coloring in the REPL
- 📜 **Multi-line input** - automatic continuation prompt for incomplete statements
- 📊 **Multiple output formats** - table, csv, json, jsonl, tsv, html
- 📤 **Data export** - INSERT statements, COPY format, file output
- 🔍 **Deep introspection** - system catalog queries for tables, indexes, functions, sequences
- 🧾 **DDL generation** - reconstruct CREATE TABLE statements from live schema
- 📝 **Script execution** - `.sql` file runner with variable substitution
- 🏁 **Single-transaction mode** - wrap script files in a transaction
- 📈 **Benchmark mode** - `--repeat N` + `--stats` for latency histograms and throughput
- 🕘 **Persistent history** - deduplication, configurable size

---

## 📦 Installation

### Prebuilt binaries

Download from the [Releases](https://github.com/jhonsferg/pgcli-rs/releases) page:

| Platform       | Binary                          |
| -------------- | ------------------------------- |
| Linux x86_64   | `pgcli-rs-linux-x86_64.tar.gz`  |
| Linux ARM64    | `pgcli-rs-linux-aarch64.tar.gz` |
| macOS x86_64   | `pgcli-rs-macos-x86_64.tar.gz`  |
| macOS ARM64    | `pgcli-rs-macos-aarch64.tar.gz` |
| Windows x86_64 | `pgcli-rs-windows-x86_64.zip`   |

### Build from source

```sh
git clone https://github.com/jhonsferg/pgcli-rs
cd pgcli-rs
cargo build --release
./target/release/pgcli --version
```

**Fully static musl binary (Linux):**

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
# Result: target/x86_64-unknown-linux-musl/release/pgcli (~7 MB, no dynamic deps)
```

---

## 🚀 Quick Start

### Interactive REPL

```sh
# Connect with individual flags
pgcli -h localhost -p 5432 -U postgres -d mydb

# Connect via URI
pgcli "postgresql://myuser:mypass@localhost:5432/mydb"

# Connect with TLS (require encrypted connection)
pgcli -h db.example.com -U postgres -d prod --require-tls

# Connect to a local socket (Unix domain socket)
pgcli -h /var/run/postgresql -U postgres -d mydb
```

Once connected you will see:

```
pgcli 0.1.0-connected to mydb at localhost:5432
Type \? for help. Type \q or press Ctrl-D to exit.

mydb=#
```

### One-shot command mode (`-c`)

```sh
# Run a SQL query and exit
pgcli -h localhost -U postgres -c "SELECT version();"

# Run a meta-command (backslash command) and exit
pgcli -h localhost -U postgres -c "\l"
pgcli -h localhost -U postgres -d mydb -c "\dt"

# List all databases
pgcli -l -h localhost -U postgres
```

### Output formats

```sh
# Table (default)
pgcli -h localhost -U postgres -d mydb -c "SELECT id, name, salary FROM employees LIMIT 5;"

# CSV-pipe-friendly
pgcli -h localhost -U postgres -d mydb --format csv \
  -c "SELECT id, name, salary FROM employees LIMIT 5;"

# JSON array
pgcli -h localhost -U postgres -d mydb --format json \
  -c "SELECT id, name FROM employees LIMIT 3;"

# JSON Lines (one object per line)
pgcli -h localhost -U postgres -d mydb --format jsonl \
  -c "SELECT id, name FROM employees LIMIT 3;"

# Tab-separated
pgcli -h localhost -U postgres -d mydb --format tsv \
  -c "SELECT id, name, salary FROM employees LIMIT 5;"

# HTML table
pgcli -h localhost -U postgres -d mydb --format html \
  -c "SELECT * FROM products LIMIT 10;" -o report.html
```

### File execution

```sh
# Execute a SQL script
pgcli -f schema.sql -h localhost -U postgres -d mydb

# Execute as a single transaction (all-or-nothing)
pgcli -f migrations/0042_add_column.sql -h localhost -U postgres -d mydb \
  --single-transaction
```

### Benchmark / profiling mode

```sh
# Run the same query 20 times and print a latency histogram
pgcli -h localhost -U postgres -d mydb --repeat 20 --stats \
  -c "SELECT count(*), avg(salary) FROM employees;"

# Run 10 iterations without printing intermediate results (stats only)
pgcli -h localhost -U postgres -d mydb --repeat 10 --stats \
  -c "SELECT * FROM orders WHERE status = 'delivered' LIMIT 1000;"
```

Sample benchmark output:

```
-- Benchmark: 20 runs -----------------------------
  Latency (ms)    min=1.899  avg=2.360  max=3.612
  Percentiles     p50=2.180  p95=3.295  p99=3.612
  Throughput      424 rows/sec
  Data xfer       14.2 KB total  (6017 KB/sec)
  Total rows      20
  Process RSS     5192 KB
------------------------------------------------
```

---

## 🔧 Full Flag Reference

### Connection flags (psql compatible)

| Flag        | Long form       | Default                      | Description                             |
| ----------- | --------------- | ---------------------------- | --------------------------------------- |
| `-h HOST`   | `--host`        | `localhost` or `$PGHOST`     | Server hostname or IP address           |
| `-p PORT`   | `--port`        | `5432` or `$PGPORT`          | Server TCP port                         |
| `-U USER`   | `--username`    | `$PGUSER` or current OS user | Database login user                     |
| `-d DBNAME` | `--dbname`      | `$PGDATABASE` or username    | Target database name                    |
| `-W`        | `--password`    | -                            | Force interactive password prompt       |
| `-w`        | `--no-password` | -                            | Never prompt for password; fail instead |

### Execution flags

| Flag      | Long form              | Description                                           |
| --------- | ---------------------- | ----------------------------------------------------- |
| `-c SQL`  | `--command`            | Execute single SQL statement or meta-command and exit |
| `-f FILE` | `--file`               | Execute a SQL script file and exit                    |
| `-o FILE` | `--output`             | Write output to FILE instead of stdout                |
| `-l`      | `--list`               | List all databases and exit                           |
| `-1`      | `--single-transaction` | Wrap `-f` script in a single transaction              |

### Output flags (psql compatible)

| Flag     | Long form            | Description                                        |
| -------- | -------------------- | -------------------------------------------------- |
| `-t`     | `--tuples-only`      | Print rows only; suppress headers and footers      |
| `-A`     | `--no-align`         | Unaligned output mode                              |
| `-H`     | `--html`             | HTML `<table>` output                              |
| `-F SEP` | `--field-separator`  | Field separator for unaligned mode (default: `\|`) |
| `-R SEP` | `--record-separator` | Record separator (default: newline)                |
| `-q`     | `--quiet`            | Suppress informational messages                    |
| `-e`     | `--echo-queries`     | Echo each query to stderr before executing         |

### TLS / SSL flags

| Flag              | Description                                             |
| ----------------- | ------------------------------------------------------- |
| `--no-tls`        | Disable TLS completely (insecure)                       |
| `--require-tls`   | Require TLS; fail if server does not support it         |
| `--tls-ca FILE`   | CA certificate file for server certificate verification |
| `--tls-cert FILE` | Client certificate file (mutual TLS)                    |
| `--tls-key FILE`  | Client private key file (mutual TLS)                    |

### pgcli extension flags

| Flag                  | Default                          | Description                                              |
| --------------------- | -------------------------------- | -------------------------------------------------------- |
| `--format FORMAT`     | `table`                          | Output format: `table` `csv` `json` `jsonl` `tsv` `html` |
| `--export FILE`       | -                                | Save result to FILE (format inferred from extension)     |
| `--timeout SECS`      | `30`                             | Connection and query timeout in seconds                  |
| `--max-rows N`        | `1000`                           | Max rows displayed in interactive mode                   |
| `--pager CMD`         | `less -RFX`                      | Override pager command                                   |
| `--no-pager`          | -                                | Disable pager output                                     |
| `--theme THEME`       | `dark`                           | Color theme: `dark` `light` `none`                       |
| `--history-file FILE` | `~/.pgcli-rs_history`            | History file path                                        |
| `--config FILE`       | `~/.config/pgcli-rs/config.toml` | Config file path                                         |
| `--repeat N`          | `1`                              | Repeat `--command` N times (benchmark mode)              |
| `--stats`             | -                                | Print latency histogram and throughput after execution   |

---

## 🔡 Meta-Commands (REPL)

### Navigation & Connection

| Command            | Description                     |
| ------------------ | ------------------------------- |
| `\q`, `\quit`      | Exit pgcli                      |
| `\c DBNAME [USER]` | Connect to a different database |
| `\conninfo`        | Show current connection details |
| `\! CMD`           | Execute a shell command         |
| `\? `, `\help`     | Show meta-command help          |

### Schema Introspection

| Command                 | Description                                                      |
| ----------------------- | ---------------------------------------------------------------- |
| `\l [PATTERN]`, `\list` | List databases                                                   |
| `\d [PATTERN]`          | Describe tables, views, and sequences matching pattern           |
| `\dt [PATTERN]`         | List tables (`schema.table` patterns supported, e.g. `\dt hr.*`) |
| `\dv [PATTERN]`         | List views                                                       |
| `\di [PATTERN]`         | List indexes                                                     |
| `\ds [PATTERN]`         | List sequences                                                   |
| `\df [PATTERN]`         | List functions                                                   |
| `\dn [PATTERN]`         | List schemas                                                     |
| `\du [PATTERN]`         | List roles and permissions                                       |

**Pattern syntax:** `*` matches anything, `?` matches a single character, `schema.name` filters by both schema and object name.

```sql
-- List all tables in the "hr" schema
\dt hr.*

-- List tables whose names start with "ord"
\dt ord*

-- Describe the employees table
\d hr.employees
```

### Display & Formatting

| Command             | Description                             |
| ------------------- | --------------------------------------- |
| `\x [on\|off]`      | Toggle expanded (vertical) display mode |
| `\timing [on\|off]` | Toggle query execution timing           |
| `\format FORMAT`    | Switch output format mid-session        |
| `\theme THEME`      | Switch color theme mid-session          |

### pgcli Extensions

| Command             | Description                                                   |
| ------------------- | ------------------------------------------------------------- |
| `\size [TABLE]`     | Show disk size of entire database or a specific table         |
| `\locks`            | Show active locks from `pg_locks`                             |
| `\activity`         | Show active queries from `pg_stat_activity`                   |
| `\introspect TABLE` | Deep introspection of a table (columns, constraints, indexes) |
| `\ddl TABLE`        | Generate `CREATE TABLE` DDL for a table                       |

### Variables

| Command            | Description                                    |
| ------------------ | ---------------------------------------------- |
| `\set [VAR VALUE]` | Set a session variable (or list all variables) |
| `\unset VAR`       | Remove a session variable                      |
| `\echo TEXT`       | Print text to stdout                           |

---

## 🔑 Authentication

pgcli supports all common PostgreSQL authentication methods:

| Method                   | Description                                               |
| ------------------------ | --------------------------------------------------------- |
| **Trust**                | No password required                                      |
| **Password (cleartext)** | Simple password over the wire (use TLS!)                  |
| **MD5**                  | Salted MD5 password hash                                  |
| **SCRAM-SHA-256**        | Modern, secure challenge-response auth (default in PG14+) |

### Password sources (priority order)

1. `--password` / `-W` flag → interactive prompt
2. `$PGPASSWORD` environment variable
3. `~/.pgpass` file (or `$PGPASSFILE`)
4. `.pgpass` record format: `hostname:port:database:username:password`

```
# ~/.pgpass example
db.example.com:5432:mydb:myuser:s3cr3t
*:5432:*:readonly:readonlypass
```

---

## 🌐 Environment Variables

pgcli respects all standard PostgreSQL environment variables:

| Variable     | Description                                                  |
| ------------ | ------------------------------------------------------------ |
| `PGHOST`     | Server hostname                                              |
| `PGPORT`     | Server port                                                  |
| `PGUSER`     | Database user                                                |
| `PGPASSWORD` | Password (prefer `~/.pgpass` for security)                   |
| `PGDATABASE` | Database name                                                |
| `PGPASSFILE` | Path to `.pgpass` file                                       |
| `PGSSLMODE`  | TLS mode: `disable` `allow` `prefer` `require` `verify-full` |
| `PAGER`      | Pager command (e.g. `less -RFX`, `more`)                     |
| `RUST_LOG`   | Log level filter (e.g. `RUST_LOG=info`)                      |

---

## ⚙️ Configuration File

Default location: `~/.config/pgcli-rs/config.toml`

```toml
[connection]
host = "localhost"
port = 5432
timeout_secs = 30

[display]
theme = "dark"           # dark | light | none
syntax_highlight = true
max_rows = 1000
border = 1
null_display = ""
expanded = "auto"        # auto | on | off

[pager]
command = "less -RFX"
enabled = true

[history]
file = "~/.pgcli-rs_history"
max_entries = 10000

[output]
format = "table"         # table | csv | json | jsonl | tsv | html
timing = true
```

---

## 🏗️ Architecture

pgcli is built on a clean layered architecture with zero C dependencies:

```
main.rs
  └-- CliArgs            (cli/args.rs)         - clap-derive argument parsing
  └-- ConnectionConfig   (connection/config.rs) - merge CLI + env + .pgpass
        └-- TlsConfig    (connection/tls.rs)    - TLS mode handling
        └-- ConnectionPool (connection/pool.rs)   - single-connection pool + reconnect
              └-- AuthHandler (protocol/auth.rs)     - SCRAM-SHA-256 / MD5 / Trust
  └-- [non-interactive] ScriptPipeline / QueryExecutor
  └-- [interactive] ReplEditor
        └-- MetaCommandDispatcher (meta/commands.rs)
        └-- QueryExecutor         (executor/query.rs)
              └-- QueryResult     (protocol/messages.rs)
                    └-- Formatter (output/formats.rs)
                          └-- Pager (output/pager.rs)
```

**Key dependencies:**

| Crate                                                                           | Role                               |
| ------------------------------------------------------------------------------- | ---------------------------------- |
| [`tokio-postgres`](https://docs.rs/tokio-postgres)                              | Pure-Rust PostgreSQL wire protocol |
| [`tokio`](https://tokio.rs)                                                     | Async runtime                      |
| [`rustls`](https://docs.rs/rustls) / [`native-tls`](https://docs.rs/native-tls) | TLS backends                       |
| [`rustyline`](https://docs.rs/rustyline)                                        | Readline/REPL input                |
| [`comfy-table`](https://docs.rs/comfy-table)                                    | Unicode table rendering            |
| [`clap`](https://docs.rs/clap)                                                  | CLI argument parsing               |
| [`tracing`](https://docs.rs/tracing)                                            | Structured logging                 |

---

## 📈 Performance

Benchmarks measured against a PostgreSQL 16 server over LAN:

| Workload                   | Platform         | p50 latency | p95 latency | Throughput    |
| -------------------------- | ---------------- | ----------- | ----------- | ------------- |
| Simple aggregate (20 runs) | Linux (loopback) | 2.2 ms      | 3.3 ms      | 424 rows/sec  |
| Simple aggregate (20 runs) | Windows (LAN)    | 12.0 ms     | 27.1 ms     | 62 rows/sec   |
| 1 000-row result (5 runs)  | Linux (loopback) | 9.9 ms      | 11.4 ms     | 99 K rows/sec |
| 1 000-row result (5 runs)  | Windows (LAN)    | 23.4 ms     | 32.2 ms     | 41 K rows/sec |
| Complex JOIN + GROUP BY    | Linux (loopback) | 14.3 ms     | 17.2 ms     | 476 rows/sec  |

Memory footprint: **< 10 MB RSS** at peak for a 1 000-row result set.

---

## 🧪 Running Tests

```sh
# Unit tests - no database required
cargo test --lib

# Lint check
cargo clippy -- -D warnings

# Format check
cargo fmt --check

# Integration tests - requires a live PostgreSQL instance
PGCLI_RS_TEST_DSN="postgresql://user:pass@localhost/testdb" \
  cargo test --features integration-tests

# Build documentation
cargo doc --no-deps --open
```

---

## 🔐 TLS Examples

```sh
# Prefer TLS (default) - upgrades if server supports it
pgcli -h db.example.com -U postgres -d mydb

# Require TLS - fail if server does not support it
pgcli -h db.example.com -U postgres -d mydb --require-tls

# Verify server certificate (verify-full)
pgcli -h db.example.com -U postgres -d mydb \
  --require-tls --tls-ca /etc/ssl/certs/my-ca.crt

# Mutual TLS (client certificate authentication)
pgcli -h db.example.com -U postgres -d mydb \
  --require-tls \
  --tls-cert ~/.config/pgcli-rs/client.crt \
  --tls-key  ~/.config/pgcli-rs/client.key \
  --tls-ca   ~/.config/pgcli-rs/ca.crt

# Via environment variable
PGSSLMODE=require pgcli -h db.example.com -U postgres -d mydb
```

---

## 📤 Data Export Examples

```sh
# Export query result to CSV file
pgcli -h localhost -U postgres -d mydb \
  -c "SELECT * FROM employees" \
  --format csv -o employees.csv

# Export to JSON
pgcli -h localhost -U postgres -d mydb \
  -c "SELECT id, name, salary FROM employees WHERE active = true" \
  --format json -o active_employees.json

# Export as SQL INSERT statements
pgcli -h localhost -U postgres -d mydb \
  -c "\export employees"

# Pipe CSV directly to another tool
pgcli -h localhost -U postgres -d mydb \
  --format csv --tuples-only \
  -c "SELECT name, salary FROM employees ORDER BY salary DESC LIMIT 100" \
  | sort -t, -k2 -rn | head -10
```

---

## 🐋 Usage in Docker / CI

Because `pgcli-rs` is a single static binary, it is ideal for containers and CI pipelines:

```dockerfile
# Multi-stage: copy the binary into a scratch container
FROM scratch
COPY --from=builder /workspace/pgcli /usr/local/bin/pgcli-rs
```

```yaml
# GitHub Actions example
- name: Run DB migration check
  run: |
    ./pgcli -h ${{ secrets.DB_HOST }} \
               -U ${{ secrets.DB_USER }} \
               -d ${{ secrets.DB_NAME }} \
               -f migrations/latest.sql \
               --single-transaction
  env:
    PGPASSWORD: ${{ secrets.DB_PASSWORD }}
```

---

## 🤝 Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, coding conventions, and the pull request process.

**Coding rules at a glance:**

- No `.unwrap()` / `.expect()` outside `main.rs`
- All `pub` items have `///` doc comments
- Every module has a `#[cfg(test)]` block
- `cargo fmt` + `cargo clippy -- -D warnings` must pass
- No libpq or C PostgreSQL bindings of any kind
- Use `tracing::error!` / `warn!` / `info!`-never `eprintln!`

---

## 📜 License

MIT License. See [LICENSE](LICENSE).

---

<p align="center">
  Built with 🦀 Rust &nbsp;|&nbsp; Pure PostgreSQL wire protocol &nbsp;|&nbsp; Zero system dependencies
</p>
