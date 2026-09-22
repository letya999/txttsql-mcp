default:
    @just --list

fmt:
    cargo fmt --all -- --check

check:
    cargo check --locked

test:
    cargo test --locked

lint:
    cargo clippy --locked --all-targets -- -D warnings

ci: fmt check lint test

docker-build:
    docker build -t txttsql-mcp:local .
