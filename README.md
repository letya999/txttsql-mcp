# txttsql-mcp

[Русский](README.ru.md) · [Quickstart](QUICKSTART.md) · [Plugin contract](docs/operations/plugins.md) · [Security](SECURITY.md)

An MCP server for read-only analytics across named PostgreSQL, CockroachDB and ClickHouse sources. Database and metadata plugins are separate executable packages with versioned manifests. Optional metadata providers include OpenMetadata and Airflow; optional local memory stores successful queries and fetched metadata.

The server exposes 13 MCP tools for source discovery, SQL validation and execution, ontology exploration, saved queries, and metadata search. Each source has its own allowlist, timeout and row cap. The SQL guard runs before the adapter; PostgreSQL and CockroachDB use read-only transactions, and ClickHouse requires a read-only account/profile. **Use a database role with server-side read-only grants.**

## Quickstart

Requirements: Rust 1.96+ and a PostgreSQL, CockroachDB or ClickHouse account with read-only grants.

```sh
git clone https://github.com/letya999/txttsql-mcp.git
cd txttsql-mcp
cp config.example.toml config.toml
# Edit config.toml: keep passwords as env/file/broker references; set allowed_schemas or allowed_tables.
cargo build --release --locked
APP_PG_PASSWORD='your-local-secret' ./target/release/txttsql-mcp --config config.toml
```

On Windows use `Copy-Item config.example.toml config.toml`, set `$env:APP_PG_PASSWORD`, and run `.\target\release\txttsql-mcp.exe --config config.toml`. The example config contains three sources; remove or configure each source you do not use. Stdout is reserved for MCP stdio, so run the process from an MCP client for tool calls. Never commit `config.toml` or a secret file.

For a client that accepts an `mcpServers` map, copy [mcp.json](mcp.json), replace the command with the absolute path to the built binary and replace the config placeholder with an absolute path. Credentials may also be loaded with `--env-file PATH` from a private file. Start with `list_sources`, `list_tables`, `describe_table`, `validate_sql`, then `execute_sql`.

```json
{
  "mcpServers": {
    "txttsql-mcp": {
      "command": "/absolute/path/to/txttsql-mcp",
      "args": ["--config", "/absolute/path/to/config.toml"]
    }
  }
}
```

## Plugins and deployment

Install database and metadata plugins independently by pointing `plugin_databases` or `plugin_metadata` at each package's `plugin.json`. The repository includes SQLite, JSON catalog and OpenMetadata examples. Plugins are trusted local executables, so install only packages you trust. See [plugin setup](docs/operations/plugins.md) and [configuration](docs/operations/configuration.md).

Docker builds are supported with the [Dockerfile](Dockerfile); mount a private config and pass secrets separately. See [quickstart](QUICKSTART.md) for the full configuration flow and [quality gates](docs/engineering/quality-gates.md) for test commands.

## Registry and license

MCP Registry name: `mcp-name: io.github.letya999/txttsql-mcp`. [server.json](server.json) describes the planned Cargo package. **It cannot be published to the official MCP Registry until version 0.1.0 is published to crates.io**; the GitHub source repository alone is not an installable registry package.

Licensed under [GNU AGPL-3.0-only](LICENSE).
