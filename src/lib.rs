pub mod config;
pub mod database;
pub mod guard;
pub mod memory;
pub mod metadata;
pub mod plugin;

use crate::{
    config::Config, database::Registry as DatabaseRegistry, memory::MemoryStore,
    metadata::Registry as MetadataRegistry, plugin::Registry as PluginRegistry,
};
use anyhow::{Context, Result};
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;
use serde_json::json;
use std::{path::Path, sync::Arc};

struct State {
    config: Config,
    databases: DatabaseRegistry,
    metadata: MetadataRegistry,
    plugins: PluginRegistry,
    memory: MemoryStore,
}

enum SourceRef<'a> {
    Builtin(&'a config::Source),
    Plugin(Arc<plugin::DatabasePlugin>),
}

impl SourceRef<'_> {
    fn policy(&self) -> guard::SqlPolicy<'_> {
        match self {
            Self::Builtin(source) => guard::SqlPolicy::from(*source),
            Self::Plugin(plugin) => plugin.policy(),
        }
    }
}

#[derive(Clone)]
pub struct McpServer {
    state: Arc<State>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SqlParams {
    /// Configured source ID.
    source: String,
    /// A single read-only SELECT statement.
    sql: String,
    /// Optional maximum rows, bounded by the source policy.
    max_rows: Option<u32>,
    /// Optional natural-language question saved with a successful query when memory is enabled.
    question: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchParams {
    text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SourceParams {
    source: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TableParams {
    source: String,
    /// Qualified schema.table name from the source allowlist.
    table: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GraphParams {
    /// joins, lineage, ontology, usage, or mart_deps.
    graph: String,
    /// Exact graph node ID.
    node: String,
    /// Neighbor depth, 1 or 2.
    hops: Option<u8>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MetadataParams {
    provider: String,
    text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MetadataDetailParams {
    provider: String,
    id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AnnotationParams {
    id: i64,
    tags: Vec<String>,
    note: String,
}

impl McpServer {
    pub fn load(path: &Path) -> Result<Self> {
        let (config, warnings) = Config::resolve(path);
        for warning in warnings {
            eprintln!("txttsql-mcp config warning: {warning}");
        }
        let plugins = PluginRegistry::new(&config, path.parent().unwrap_or(Path::new(".")))?;
        let mut databases = DatabaseRegistry::new(&config.sources)?;
        for plugin in plugins.databases() {
            databases.insert(plugin.config.id.clone(), plugin)?;
        }
        let mut metadata =
            MetadataRegistry::new(config.openmetadata.clone(), config.airflow.clone())?;
        for plugin in plugins.metadata() {
            metadata.insert(plugin.id.clone(), plugin)?;
        }
        let memory = MemoryStore::new(&config.memory);
        Ok(Self {
            state: Arc::new(State {
                config,
                databases,
                metadata,
                plugins,
                memory,
            }),
        })
    }

    fn source(&self, id: &str) -> Result<SourceRef<'_>> {
        if let Some(source) = self
            .state
            .config
            .sources
            .iter()
            .find(|source| source.id == id)
        {
            return Ok(SourceRef::Builtin(source));
        }
        self.state
            .plugins
            .database(id)
            .map(SourceRef::Plugin)
            .context("unknown source")
    }
}

fn error(message: &str) -> String {
    json!({"ok": false, "error": message}).to_string()
}

#[tool_router(server_handler)]
impl McpServer {
    #[tool(
        description = "List configured database and metadata sources without revealing credentials"
    )]
    fn list_sources(&self) -> String {
        let mut sources: Vec<_> = self.state.config.sources.iter().map(|source| json!({"id": source.id, "kind": format!("{:?}", source.kind).to_lowercase(), "max_rows": source.max_rows})).collect();
        sources.extend(self.state.plugins.databases().map(|plugin| json!({"id": plugin.config.id, "kind": "plugin", "plugin": plugin.plugin_id(), "max_rows": plugin.config.max_rows})));
        json!({"sources": sources, "metadata": self.state.metadata.names(), "query_memory": self.state.memory.enabled()}).to_string()
    }

    #[tool(
        description = "List accessible tables for one source, including tables in allowed schemas"
    )]
    async fn list_tables(&self, Parameters(params): Parameters<SourceParams>) -> String {
        let source = match self.source(&params.source) {
            Ok(source) => source,
            Err(err) => return error(&err.to_string()),
        };
        if let SourceRef::Plugin(plugin) = &source {
            return match plugin.list_tables().await {
                Ok(tables) => {
                    let discovered: Vec<_> = tables.iter().filter_map(|name| name.split_once('.').map(|(schema, table)| json!({"table_schema": schema, "table_name": table}))).collect();
                    json!({"ok": true, "configured_tables": plugin.config.allowed_tables, "discovered": discovered, "truncated": false}).to_string()
                }
                Err(_) => error("table discovery failed"),
            };
        }
        let SourceRef::Builtin(source) = source else {
            unreachable!()
        };
        let configured = &source.allowed_tables;
        if source.allowed_schemas.is_empty() {
            return json!({"ok": true, "tables": configured, "truncated": false}).to_string();
        }
        let schemas = source
            .allowed_schemas
            .iter()
            .map(|schema| {
                let value = if source.kind == config::DatabaseKind::Clickhouse {
                    schema.clone()
                } else {
                    schema.to_ascii_lowercase()
                };
                format!("'{value}'")
            })
            .collect::<Vec<_>>()
            .join(",");
        let sql = match source.kind {
            config::DatabaseKind::Clickhouse => format!(
                "SELECT database AS table_schema, name AS table_name FROM system.tables WHERE database IN ({schemas}) ORDER BY database, name"
            ),
            _ => format!(
                "SELECT table_schema, table_name FROM information_schema.tables WHERE table_schema IN ({schemas}) AND table_type IN ('BASE TABLE','VIEW') ORDER BY table_schema, table_name"
            ),
        };
        let approved = guard::ApprovedQuery {
            sql,
            tables: vec![],
            row_cap: 1000,
        };
        let Some(adapter) = self.state.databases.get(&params.source) else {
            return error("database adapter is unavailable");
        };
        match adapter.execute(&approved).await {
            Ok(result) => json!({"ok": true, "configured_tables": configured, "discovered": result.rows, "truncated": result.truncated}).to_string(),
            Err(_) => error("table discovery failed"),
        }
    }

    #[tool(description = "Describe columns of an allowed schema.table without reading its data")]
    async fn describe_table(&self, Parameters(params): Parameters<TableParams>) -> String {
        let source = match self.source(&params.source) {
            Ok(source) => source,
            Err(err) => return error(&err.to_string()),
        };
        if !guard::table_allowed_policy(&source.policy(), &params.table) {
            return error("table is outside the source allowlist");
        }
        if let SourceRef::Plugin(plugin) = &source {
            return match plugin.describe_table(&params.table).await {
                Ok(result) => json!({"ok": true, "table": params.table, "columns": result["columns"], "truncated": result["truncated"].as_bool().unwrap_or(false)}).to_string(),
                Err(_) => error("table description failed"),
            };
        }
        let SourceRef::Builtin(source) = source else {
            unreachable!()
        };
        let Some((schema, table)) = params.table.split_once('.') else {
            return error("table must use schema.table name");
        };
        let (schema, table) = if source.kind == config::DatabaseKind::Clickhouse {
            (schema.to_owned(), table.to_owned())
        } else {
            (schema.to_ascii_lowercase(), table.to_ascii_lowercase())
        };
        let sql = match source.kind {
            config::DatabaseKind::Clickhouse => format!(
                "SELECT name AS column_name, type AS data_type FROM system.columns WHERE database = '{schema}' AND table = '{table}' ORDER BY position"
            ),
            _ => format!(
                "SELECT column_name, data_type, is_nullable FROM information_schema.columns WHERE table_schema = '{schema}' AND table_name = '{table}' ORDER BY ordinal_position"
            ),
        };
        let approved = guard::ApprovedQuery {
            sql,
            tables: vec![],
            row_cap: 1000,
        };
        let Some(adapter) = self.state.databases.get(&params.source) else {
            return error("database adapter is unavailable");
        };
        match adapter.execute(&approved).await {
            Ok(result) if result.rows.is_empty() => error("table not found or columns are not visible"),
            Ok(result) => json!({"ok": true, "table": params.table, "columns": result.rows, "truncated": result.truncated}).to_string(),
            Err(_) => error("table description failed"),
        }
    }

    #[tool(
        description = "Validate one read-only SQL query against the dialect and source allowlist without connecting to the database"
    )]
    fn validate_sql(&self, Parameters(params): Parameters<SqlParams>) -> String {
        match self.source(&params.source).and_then(|source| guard::validate_policy(&source.policy(), &params.sql, params.max_rows)) {
            Ok(query) => json!({"ok": true, "sql": query.sql, "tables": query.tables, "max_rows": query.row_cap}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(
        description = "Execute a validated SELECT in a read-only transaction or ClickHouse read-only account, with timeout and row cap"
    )]
    async fn execute_sql(&self, Parameters(params): Parameters<SqlParams>) -> String {
        if params
            .question
            .as_ref()
            .is_some_and(|question| question.len() > 2000)
        {
            return error("question is too long");
        }
        let source = match self.source(&params.source) {
            Ok(source) => source,
            Err(err) => return error(&err.to_string()),
        };
        let approved = match guard::validate_policy(&source.policy(), &params.sql, params.max_rows)
        {
            Ok(query) => query,
            Err(err) => return error(&err.to_string()),
        };
        let Some(database) = self.state.databases.get(&params.source) else {
            return error("database adapter is unavailable");
        };
        match database.execute(&approved).await {
            Ok(result) => {
                let memory_enabled = self.state.memory.enabled();
                let memory_saved = memory_enabled
                    && self
                        .state
                        .memory
                        .record(
                            &params.source,
                            params.question.as_deref().unwrap_or(""),
                            &params.sql,
                            &approved.tables,
                        )
                        .await
                        .is_ok();
                json!({"ok": true, "source": params.source, "tables": approved.tables, "result": result, "memory_saved": memory_saved}).to_string()
            }
            Err(_) => error("database query failed"),
        }
    }

    #[tool(
        description = "Search the configured ontology Markdown tree for business definitions, rules and lineage"
    )]
    async fn find_context(&self, Parameters(params): Parameters<SearchParams>) -> String {
        match self.state.memory.context(&params.text).await {
            Ok(hits) => json!({"ok": true, "hits": hits}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(
        description = "Explore typed ontology, join and lineage relationships from the configured txttsql graph files"
    )]
    async fn explore_graph(&self, Parameters(params): Parameters<GraphParams>) -> String {
        match self
            .state
            .memory
            .graph(&params.graph, &params.node, params.hops.unwrap_or(1))
            .await
        {
            Ok(result) => json!({"ok": true, "result": result}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(description = "Find previously successful SQL queries, if query memory is enabled")]
    async fn search_saved_queries(&self, Parameters(params): Parameters<SearchParams>) -> String {
        match self.state.memory.search(&params.text).await {
            Ok(queries) => json!({"ok": true, "queries": queries}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(description = "Attach validated tags and a note to a saved successful query")]
    async fn annotate_saved_query(
        &self,
        Parameters(params): Parameters<AnnotationParams>,
    ) -> String {
        match self
            .state
            .memory
            .annotate(params.id, params.tags, params.note)
            .await
        {
            Ok(()) => json!({"ok": true}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(
        description = "Search cached OpenMetadata entities and Airflow DAGs when memory is enabled"
    )]
    async fn search_saved_metadata(&self, Parameters(params): Parameters<SearchParams>) -> String {
        match self.state.memory.search_metadata(&params.text).await {
            Ok(entities) => json!({"ok": true, "entities": entities}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(description = "Attach validated tags and a note to a cached metadata entity")]
    async fn annotate_saved_metadata(
        &self,
        Parameters(params): Parameters<AnnotationParams>,
    ) -> String {
        match self
            .state
            .memory
            .annotate_metadata(params.id, params.tags, params.note)
            .await
        {
            Ok(()) => json!({"ok": true}).to_string(),
            Err(err) => error(&err.to_string()),
        }
    }

    #[tool(
        description = "Search OpenMetadata tables or Airflow DAG identifiers, when the provider is configured"
    )]
    async fn search_metadata(&self, Parameters(params): Parameters<MetadataParams>) -> String {
        let Some(provider) = self.state.metadata.get(&params.provider) else {
            return error("unknown metadata provider");
        };
        match provider.search(&params.text).await {
            Ok(result) => json!({"ok": true, "result": result}).to_string(),
            Err(_) => error("metadata service request failed"),
        }
    }

    #[tool(
        description = "Read an OpenMetadata table by fully qualified name or an Airflow DAG with tasks"
    )]
    async fn metadata_detail(
        &self,
        Parameters(params): Parameters<MetadataDetailParams>,
    ) -> String {
        let Some(provider) = self.state.metadata.get(&params.provider) else {
            return error("unknown metadata provider");
        };
        match provider.detail(&params.id).await {
            Ok(result) => {
                let memory_saved = self
                    .state
                    .memory
                    .record_metadata(&params.provider, &params.id, &result)
                    .await
                    .unwrap_or(false);
                json!({"ok": true, "result": result, "memory_saved": memory_saved}).to_string()
            }
            Err(_) => error("metadata service request failed"),
        }
    }
}
