use crate::{
    config::{DatabaseKind, Source},
    guard::ApprovedQuery,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Serialize;
use serde_json::Value;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
    types::Json,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

#[derive(Serialize)]
pub struct QueryResult {
    pub rows: Vec<Value>,
    pub truncated: bool,
    pub elapsed_ms: u128,
}

#[async_trait]
pub trait DatabaseAdapter: Send + Sync {
    async fn execute(&self, query: &ApprovedQuery) -> Result<QueryResult>;
}

pub struct Registry {
    adapters: HashMap<String, Arc<dyn DatabaseAdapter>>,
}

impl Registry {
    pub fn new(sources: &[Source]) -> Result<Self> {
        let mut adapters: HashMap<String, Arc<dyn DatabaseAdapter>> = HashMap::new();
        for source in sources {
            let adapter: Arc<dyn DatabaseAdapter> = match source.kind {
                DatabaseKind::Postgres | DatabaseKind::Cockroach => Arc::new(PostgresAdapter {
                    source: source.clone(),
                    pool: Mutex::new(None),
                }),
                DatabaseKind::Clickhouse => Arc::new(ClickHouseAdapter::new(source.clone())?),
            };
            adapters.insert(source.id.clone(), adapter);
        }
        Ok(Self { adapters })
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn DatabaseAdapter>> {
        self.adapters.get(id).cloned()
    }
}

struct PostgresAdapter {
    source: Source,
    pool: Mutex<Option<(Instant, PgPool)>>,
}

impl PostgresAdapter {
    async fn pool(&self) -> Result<PgPool> {
        let mut state = self.pool.lock().await;
        if let Some((created, pool)) = state.as_ref()
            && created.elapsed() < Duration::from_secs(300)
        {
            return Ok(pool.clone());
        }
        let password = self.source.password.resolve().await?;
        let user = match &self.source.user_secret {
            Some(reference) => reference.resolve().await?.expose_secret().to_owned(),
            None => self.source.user.clone(),
        };
        let options = PgConnectOptions::new()
            .host(&self.source.host)
            .port(self.source.port)
            .database(&self.source.database)
            .username(&user)
            .password(password.expose_secret())
            .ssl_mode(if self.source.allow_insecure {
                PgSslMode::Disable
            } else {
                PgSslMode::VerifyFull
            });
        let pool = PgPoolOptions::new()
            .max_connections(self.source.max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .idle_timeout(Duration::from_secs(60))
            .connect_with(options)
            .await
            .context("database connection failed")?;
        *state = Some((Instant::now(), pool.clone()));
        Ok(pool)
    }
}

#[async_trait]
impl DatabaseAdapter for PostgresAdapter {
    async fn execute(&self, query: &ApprovedQuery) -> Result<QueryResult> {
        let pool = self.pool().await?;
        let start = Instant::now();
        let mut transaction = pool.begin().await.context("could not start transaction")?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *transaction)
            .await
            .context("read-only transaction unavailable")?;
        let timeout_ms = self.source.timeout_seconds * 1000;
        sqlx::query(&format!("SET LOCAL statement_timeout = '{timeout_ms}ms'"))
            .execute(&mut *transaction)
            .await
            .context("could not set statement timeout")?;
        let sql = format!(
            "SELECT row_to_json(_result) FROM ({}) AS _result",
            query.sql
        );
        let rows = sqlx::query_scalar::<_, Json<Value>>(&sql)
            .fetch_all(&mut *transaction)
            .await
            .context("query execution failed")?;
        transaction
            .rollback()
            .await
            .context("could not roll back read-only transaction")?;
        let truncated = rows.len() > query.row_cap as usize;
        Ok(QueryResult {
            rows: rows
                .into_iter()
                .take(query.row_cap as usize)
                .map(|row| row.0)
                .collect(),
            truncated,
            elapsed_ms: start.elapsed().as_millis(),
        })
    }
}

struct ClickHouseAdapter {
    source: Source,
    client: reqwest::Client,
    url: reqwest::Url,
    permits: Semaphore,
}

impl ClickHouseAdapter {
    fn new(source: Source) -> Result<Self> {
        let scheme = if source.allow_insecure {
            "http"
        } else {
            "https"
        };
        let url = reqwest::Url::parse(&format!("{scheme}://{}:{}/", source.host, source.port))
            .context("invalid ClickHouse host")?;
        if url.host_str() != Some(source.host.as_str()) || !url.username().is_empty() {
            bail!("invalid ClickHouse host")
        }
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(source.max_connections as usize)
            .timeout(Duration::from_secs(source.timeout_seconds + 5))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            permits: Semaphore::new(source.max_connections as usize),
            source,
            client,
            url,
        })
    }
}

#[async_trait]
impl DatabaseAdapter for ClickHouseAdapter {
    async fn execute(&self, query: &ApprovedQuery) -> Result<QueryResult> {
        let _permit = self
            .permits
            .acquire()
            .await
            .context("ClickHouse source is closed")?;
        let password = self.source.password.resolve().await?;
        let user = match &self.source.user_secret {
            Some(reference) => reference.resolve().await?.expose_secret().to_owned(),
            None => self.source.user.clone(),
        };
        let start = Instant::now();
        let mut url = self.url.clone();
        url.query_pairs_mut()
            .append_pair("database", &self.source.database)
            .append_pair("default_format", "JSON")
            .append_pair("allow_ddl", "0")
            .append_pair("readonly", "1")
            .append_pair("allow_introspection_functions", "0")
            .append_pair(
                "max_execution_time",
                &self.source.timeout_seconds.to_string(),
            )
            .append_pair("max_result_rows", &(query.row_cap + 1).to_string())
            .append_pair("max_result_bytes", "16777216")
            .append_pair("result_overflow_mode", "throw");
        let mut response = self
            .client
            .post(url)
            .basic_auth(&user, Some(password.expose_secret()))
            .body(query.sql.clone())
            .send()
            .await
            .context("ClickHouse request failed")?;
        if !response.status().is_success() {
            bail!("ClickHouse rejected the query (HTTP {})", response.status())
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("ClickHouse response failed")?
        {
            if body.len() + chunk.len() > 16 * 1024 * 1024 {
                bail!("ClickHouse response exceeds 16 MB")
            }
            body.extend_from_slice(&chunk);
        }
        let payload: Value = serde_json::from_slice(&body).context("invalid ClickHouse JSON")?;
        let rows = payload
            .get("data")
            .and_then(Value::as_array)
            .context("ClickHouse response has no data array")?;
        let truncated = rows.len() > query.row_cap as usize;
        Ok(QueryResult {
            rows: rows.iter().take(query.row_cap as usize).cloned().collect(),
            truncated,
            elapsed_ms: start.elapsed().as_millis(),
        })
    }
}
