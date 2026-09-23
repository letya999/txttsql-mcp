# txttsql-mcp

Rust MCP server for guarded analytics across named PostgreSQL, CockroachDB and ClickHouse sources. It builds on the dialect guard, table catalog, query memory and ontology layout of the sibling `txttsql` project, and takes the source/tool separation of [MCP Toolbox](https://github.com/googleapis/mcp-toolbox) and the compact multi-database guardrail approach of [DBHub](https://github.com/bytebase/dbhub).

Each source has its own identity, allowlist, timeout, row cap and connection pool. PostgreSQL and CockroachDB queries execute in read-only transactions. ClickHouse queries use an account that must have a read-only profile. The MCP surface is stdio, implemented with the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).

Separately packaged database and metadata plugins can be installed through versioned manifests without rebuilding the server. See the [plugin contract and examples](docs/operations/plugins.md).

## Start

1. Copy `config.example.toml` to `config.toml` and replace the placeholder hosts and credential references.
2. Grant the configured database users read-only access to exactly the intended schemas and tables.
3. Run `cargo run --locked -- --config config.toml`, or add the binary and the config path to your MCP client.

Credentials are resolved from environment variables, mounted secret files or a configured broker executable. The broker receives fixed argument strings and returns one credential on stdout; no shell is involved. A source may use a static `user` or a rotating `user_secret`. The config must be trusted and should contain no plaintext secrets. PostgreSQL/CockroachDB pools refresh every five minutes so new credentials can be picked up; ClickHouse and metadata tokens resolve per request.

`list_sources`, `list_tables`, `describe_table`, `validate_sql`, `execute_sql`, `find_context`, `explore_graph`, `search_saved_queries`, `annotate_saved_query`, `search_metadata`, `metadata_detail`, `search_saved_metadata` and `annotate_saved_metadata` are the current tools. Table discovery and column descriptions respect the source allowlist. Memory is off by default. It records a query only after successful execution and caches fetched metadata details with separate tags and notes. `find_context` and `explore_graph` can read the Markdown ontology and typed relationship graphs from an existing `txttsql/memory` tree mounted read-only. OpenMetadata and Airflow connections are optional; Airflow 3 API v2 is the default, with v1 and Basic Auth available for Airflow 2.

## Docker

```sh
docker build -t txttsql-mcp:local .
docker run --rm -i --env APP_PG_PASSWORD --mount type=bind,src=/absolute/path/config.toml,dst=/app/config.toml,readonly txttsql-mcp:local
```

Mount `/data` writable only if query memory is enabled. A new Docker named volume inherits the image's private `/data` permissions (`0700`); for a bind mount, grant UID `65532` private write access to the host directory. Mount `/ontology` read-only to use an existing `txttsql/memory` directory. Configure all three sources in one config; the server opens the selected source on demand.

## Documentation

The structure follows the sibling `memory_bank_setup` template: `docs/` describes current behavior, `specs/active/` holds requirements, `docs/adr/` records decisions and `.work/` tracks ongoing work. See [docs/index.md](docs/index.md) and [QUICKSTART.md](QUICKSTART.md).
