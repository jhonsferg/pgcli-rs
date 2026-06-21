# Contributing to pgcli-rs

## Development Setup

You need a stable Rust toolchain (1.75+):

```sh
rustup update stable
```

Clone the repo and build:

```sh
git clone https://github.com/pgcli-rs/pgcli-rs
cd pgcli-rs
cargo build
```

## Running Tests

Unit tests (no database required):

```sh
cargo test --lib
```

Integration tests (requires a live PostgreSQL instance):

```sh
PGCLI_RS_TEST_DSN="postgresql://user:pass@localhost/testdb" \
  cargo test --features integration-tests
```

## Code Quality

All PRs must pass:

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test --lib
cargo doc --no-deps
```

## Branch Naming

- `feat/short-description`-new features
- `fix/short-description`-bug fixes
- `docs/short-description`-documentation only
- `chore/short-description`-tooling, CI, deps

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(output): add JSONL export format
fix(auth): handle SCRAM-SHA-256 with special chars in password
docs(readme): update flag reference for --format
chore(ci): add aarch64 to release matrix
```

## PR Requirements

- All tests pass.
- `cargo clippy -- -D warnings` is clean.
- `cargo fmt --check` passes.
- Doc comments on all new `pub` items.
- Unit tests added for new logic.

## Adding a New Meta-Command

1. Add a match arm in `src/meta/commands.rs` `MetaCommandDispatcher::dispatch()`.
2. Return `MetaResult::Query(sql)` for catalog queries or `MetaResult::Output(s)` for static output.
3. Add a unit test in the `#[cfg(test)]` block.
4. Document the command in the `help_text()` function.

## Adding a New Export Format

1. Create `src/export/yourformat.rs` implementing the export logic.
2. Add `pub mod yourformat;` in `src/export/mod.rs`.
3. Add the format variant to `OutputFormat` in `src/output/formats.rs`.
4. Wire it into `format_result()` in the same file.
5. Add unit tests.
