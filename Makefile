# pgcli-rs development Makefile
#
# Quick reference:
#   make build          - debug build
#   make release        - optimized release build
#   make test           - unit tests only (no DB needed)
#   make lint           - clippy + fmt check
#   make compat-up      - start all PostgreSQL containers (14-18)
#   make compat-test    - run integration tests against all versions
#   make compat-down    - stop and remove containers + volumes
#   make compat-one V=17 - test against a single version

.PHONY: build release test lint doc \
        compat-up compat-down compat-test compat-one compat-logs \
        clean

# -- Build --------------------------------------------------------------------

build:
	cargo build

release:
	cargo build --release

# -- Tests -------------------------------------------------------------------

test:
	cargo test --lib

# Run integration tests against an already-running DB (DSN must be set).
# Example: make integration DSN="postgresql://user:pass@host/db"
integration:
	PGCLI_RS_TEST_DSN="$(DSN)" cargo test --features integration-tests --test integration_tests

# -- Quality -----------------------------------------------------------------

lint:
	cargo fmt --check
	cargo clippy -- -D warnings

doc:
	cargo doc --no-deps --open

# -- Compatibility matrix (Docker) --------------------------------------------

# Start all PostgreSQL containers in the background.
compat-up:
	docker compose up -d
	@echo "Containers started. Ports: PG14=5414 PG15=5415 PG16=5416 PG17=5417 PG18=5418"
	@echo "Run 'make compat-test' once all containers are healthy."

# Stop and remove containers + named volumes (clean slate).
compat-down:
	docker compose down -v

# Run the full compatibility test suite (all versions sequentially).
compat-test:
	bash scripts/test-compat.sh

# Test a single version. Usage: make compat-one V=17
compat-one:
	bash scripts/test-compat.sh --versions $(V)

# Stop on first failure.
compat-test-strict:
	bash scripts/test-compat.sh --stop-on-fail

# Show logs for a specific version. Usage: make compat-logs V=17
compat-logs:
	docker compose logs pg$(V) --follow

# -- Misc --------------------------------------------------------------------

clean:
	cargo clean
