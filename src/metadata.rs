use crate::config::{ApiSource, add_ca_file};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};

#[async_trait]
pub trait MetadataAdapter: Send + Sync {
    async fn search(&self, text: &str) -> Result<Value>;
    async fn detail(&self, id: &str) -> Result<Value>;
}

pub struct Registry {
    adapters: HashMap<String, Arc<dyn MetadataAdapter>>,
}

impl Registry {
    pub fn new(openmetadata: Option<ApiSource>, airflow: Option<ApiSource>) -> Result<Self> {
        let mut adapters: HashMap<String, Arc<dyn MetadataAdapter>> = HashMap::new();
        if let Some(config) = openmetadata {
            adapters.insert(
                "openmetadata".into(),
                Arc::new(OpenMetadata {
                    api: ApiClient::new(config)?,
                }),
            );
        }
        if let Some(config) = airflow {
            let version = config.api_version.unwrap_or(2);
            adapters.insert(
                "airflow".into(),
                Arc::new(Airflow {
                    api: ApiClient::new(config)?,
                    version,
                }),
            );
        }
        Ok(Self { adapters })
    }
    pub fn get(&self, name: &str) -> Option<Arc<dyn MetadataAdapter>> {
        self.adapters.get(name).cloned()
    }
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.adapters.keys().cloned().collect();
        names.sort_unstable();
        names
    }

    pub fn insert(&mut self, id: String, adapter: Arc<dyn MetadataAdapter>) -> Result<()> {
        if self.adapters.insert(id, adapter).is_some() {
            bail!("duplicate metadata adapter")
        }
        Ok(())
    }
}

struct ApiClient {
    base: reqwest::Url,
    config: ApiSource,
    client: reqwest::Client,
}

impl ApiClient {
    fn new(config: ApiSource) -> Result<Self> {
        let base = reqwest::Url::parse(&config.url)?;
        let builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none());
        let client = add_ca_file(builder, config.ca_file.as_deref())?.build()?;
        Ok(Self {
            base,
            config,
            client,
        })
    }

    async fn get(&self, path: &[&str], query: &[(&str, &str)]) -> Result<Value> {
        let mut url = self.base.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| anyhow::anyhow!("invalid metadata base URL"))?;
            segments.pop_if_empty();
            for segment in path {
                segments.push(segment);
            }
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query.iter().copied());
        }
        let request = self.client.get(url);
        let request = if let Some(token) = &self.config.token {
            request.bearer_auth(token.resolve().await?.expose_secret())
        } else {
            let password = self
                .config
                .basic_password
                .as_ref()
                .context("basic password missing")?
                .resolve()
                .await?;
            request.basic_auth(
                self.config
                    .basic_user
                    .as_ref()
                    .context("basic user missing")?,
                Some(password.expose_secret()),
            )
        };
        let mut response = request.send().await.context("metadata request failed")?;
        if !response.status().is_success() {
            bail!("metadata service returned HTTP {}", response.status())
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.context("metadata response failed")? {
            if body.len() + chunk.len() > 4 * 1024 * 1024 {
                bail!("metadata response exceeds 4 MB")
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).context("metadata response is not JSON")
    }
}

struct OpenMetadata {
    api: ApiClient,
}

#[async_trait]
impl MetadataAdapter for OpenMetadata {
    async fn search(&self, text: &str) -> Result<Value> {
        if text.trim().is_empty() || text.len() > 200 {
            bail!("search text must be 1-200 characters")
        }
        self.api
            .get(
                &["api", "v1", "search", "query"],
                &[("q", text), ("index", "table_search_index"), ("size", "20")],
            )
            .await
    }
    async fn detail(&self, id: &str) -> Result<Value> {
        if id.is_empty() || id.len() > 300 {
            bail!("invalid table name")
        }
        self.api
            .get(
                &["api", "v1", "tables", "name", id],
                &[("fields", "columns,tags,owners")],
            )
            .await
    }
}

struct Airflow {
    api: ApiClient,
    version: u8,
}

#[async_trait]
impl MetadataAdapter for Airflow {
    async fn search(&self, text: &str) -> Result<Value> {
        if text.len() > 200 {
            bail!("search text is too long")
        }
        let version = if self.version == 1 { "v1" } else { "v2" };
        let needle = text.to_lowercase();
        let mut matches = Vec::new();
        let mut scanned = 0;
        let mut incomplete = true;
        for _ in 0..10 {
            let offset = scanned.to_string();
            let result = self
                .api
                .get(
                    &["api", version, "dags"],
                    &[
                        ("limit", "100"),
                        ("offset", &offset),
                        ("order_by", "dag_id"),
                    ],
                )
                .await?;
            let dags = result
                .get("dags")
                .and_then(Value::as_array)
                .context("Airflow response has no dags")?;
            let total_entries = result.get("total_entries").and_then(Value::as_u64);
            scanned += dags.len();
            matches.extend(
                dags.iter()
                    .filter(|dag| {
                        dag.get("dag_id")
                            .and_then(Value::as_str)
                            .is_some_and(|id| id.to_lowercase().contains(&needle))
                    })
                    .take(20 - matches.len())
                    .cloned(),
            );
            if matches.len() == 20 {
                break;
            }
            if dags.is_empty() || total_entries.is_some_and(|total| scanned as u64 >= total) {
                incomplete = false;
                break;
            }
        }
        Ok(serde_json::json!({"dags": matches, "scanned": scanned, "incomplete": incomplete}))
    }
    async fn detail(&self, id: &str) -> Result<Value> {
        if id.is_empty() || id.len() > 200 {
            bail!("invalid DAG id")
        }
        let version = if self.version == 1 { "v1" } else { "v2" };
        let dag = self.api.get(&["api", version, "dags", id], &[]).await?;
        let tasks = self
            .api
            .get(&["api", version, "dags", id, "tasks"], &[])
            .await?;
        let tasks = tasks
            .get("tasks")
            .and_then(Value::as_array)
            .context("Airflow response has no tasks")?;
        let mut edges = Vec::new();
        for task in tasks {
            if let (Some(from), Some(downstream)) = (
                task.get("task_id").and_then(Value::as_str),
                task.get("downstream_task_ids").and_then(Value::as_array),
            ) {
                for to in downstream.iter().filter_map(Value::as_str) {
                    edges.push(serde_json::json!({"from": from, "to": to}));
                }
            }
        }
        Ok(serde_json::json!({ "dag": dag, "tasks": tasks, "edges": edges }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecretRef;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn openmetadata_and_airflow_routes_and_dag_edges() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let size = stream.read(&mut chunk).await.unwrap();
                    assert!(size > 0);
                    request.extend_from_slice(&chunk[..size]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                assert!(
                    request
                        .to_lowercase()
                        .contains("authorization: bearer testtoken")
                );
                let line = request.lines().next().unwrap();
                let body = if line.starts_with("GET /api/v2/dags?") {
                    if line.contains("offset=0") {
                        r#"{"dags":[{"dag_id":"payments"}],"total_entries":2}"#
                    } else {
                        assert!(line.contains("offset=1"));
                        r#"{"dags":[{"dag_id":"payments_archive"}],"total_entries":2}"#
                    }
                } else if line.starts_with("GET /api/v2/dags/payments/tasks ") {
                    r#"{"tasks":[{"task_id":"extract","downstream_task_ids":["load"]},{"task_id":"load","downstream_task_ids":[]}]}"#
                } else if line.starts_with("GET /api/v2/dags/payments ") {
                    r#"{"dag_id":"payments"}"#
                } else if line.starts_with("GET /api/v1/search/query?") {
                    r#"{"hits":{"hits":[]}}"#
                } else if line.starts_with("GET /api/v1/tables/name/service.db.schema.orders?") {
                    let url = reqwest::Url::parse(&format!(
                        "http://localhost{}",
                        line.split_whitespace().nth(1).unwrap()
                    ))
                    .unwrap();
                    assert_eq!(
                        url.query_pairs()
                            .find(|(key, _)| key == "fields")
                            .unwrap()
                            .1,
                        "columns,tags,owners"
                    );
                    r#"{"name":"orders"}"#
                } else {
                    panic!("unexpected request: {line}")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let token_path = std::env::temp_dir().join(format!("txttsql-token-{}", std::process::id()));
        std::fs::write(&token_path, "testtoken").unwrap();
        let source = |version| ApiSource {
            url: format!("http://127.0.0.1:{port}"),
            token: Some(SecretRef::File {
                path: token_path.clone(),
            }),
            basic_user: None,
            basic_password: None,
            allow_insecure: true,
            ca_file: None,
            api_version: version,
        };
        let registry = Registry::new(Some(source(None)), Some(source(Some(2)))).unwrap();
        let airflow = registry.get("airflow").unwrap();
        let dags = airflow.search("pay").await.unwrap();
        assert_eq!(dags["dags"][0]["dag_id"], "payments");
        assert_eq!(dags["dags"][1]["dag_id"], "payments_archive");
        assert_eq!(dags["scanned"], 2);
        assert_eq!(dags["incomplete"], false);
        assert_eq!(
            airflow.detail("payments").await.unwrap()["edges"][0]["to"],
            "load"
        );
        let openmetadata = registry.get("openmetadata").unwrap();
        assert!(openmetadata.search("orders").await.unwrap()["hits"].is_object());
        assert_eq!(
            openmetadata
                .detail("service.db.schema.orders")
                .await
                .unwrap()["name"],
            "orders"
        );
        server.await.unwrap();
        std::fs::remove_file(token_path).unwrap();
    }

    #[tokio::test]
    async fn airflow_v1_accepts_basic_auth_from_secret_file() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            let size = stream.read(&mut buffer).await.unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]).to_lowercase();
            assert!(request.starts_with("get /api/v1/dags?"));
            assert!(request.contains("authorization: basic cmvhzgvyonrlc3r0b2tlbg=="));
            let body = r#"{"dags":[]}"#;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let path = std::env::temp_dir().join(format!("txttsql-basic-{}", std::process::id()));
        std::fs::write(&path, "testtoken").unwrap();
        let config = ApiSource {
            url: format!("http://127.0.0.1:{port}"),
            token: None,
            basic_user: Some("reader".into()),
            basic_password: Some(SecretRef::File { path: path.clone() }),
            allow_insecure: true,
            ca_file: None,
            api_version: Some(1),
        };
        let registry = Registry::new(None, Some(config)).unwrap();
        assert!(
            registry.get("airflow").unwrap().search("x").await.unwrap()["dags"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        server.await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
