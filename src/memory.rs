use crate::config::Memory;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct MemoryStore {
    path: Option<PathBuf>,
    ontology_path: Option<PathBuf>,
}

#[derive(Serialize)]
pub struct SavedQuery {
    pub id: i64,
    pub source: String,
    pub question: String,
    pub sql: String,
    pub tables: Vec<String>,
    pub tags: Vec<String>,
    pub note: String,
}

#[derive(Serialize)]
pub struct SavedMetadata {
    pub id: i64,
    pub provider: String,
    pub key: String,
    pub payload: Value,
    pub tags: Vec<String>,
    pub note: String,
}

#[derive(Serialize)]
pub struct ContextHit {
    pub path: String,
    pub excerpt: String,
}

impl MemoryStore {
    pub fn new(config: &Memory) -> Self {
        Self {
            path: if config.enabled {
                config.path.clone()
            } else {
                None
            },
            ontology_path: config.ontology_path.clone(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.path.is_some()
    }

    pub async fn record(
        &self,
        source: &str,
        question: &str,
        sql: &str,
        tables: &[String],
    ) -> Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        if question.len() > 2000 {
            bail!("question is too long")
        }
        let (source, question, sql, tables) = (
            source.to_owned(),
            question.to_owned(),
            sql.to_owned(),
            serde_json::to_string(tables)?,
        );
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&path)?;
            conn.execute("INSERT INTO queries (source, question, sql, tables) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(source, sql) DO UPDATE SET question=excluded.question", params![source, question, sql, tables])?;
            Ok(())
        }).await??;
        Ok(())
    }

    pub async fn search(&self, text: &str) -> Result<Vec<SavedQuery>> {
        let Some(path) = self.path.clone() else {
            bail!("query memory is disabled")
        };
        let text = text.chars().take(200).collect::<String>();
        tokio::task::spawn_blocking(move || -> Result<Vec<SavedQuery>> {
            let conn = open(&path)?;
            let pattern = escape_like(&text);
            let mut stmt = conn.prepare("SELECT id, source, question, sql, tables, tags, note FROM queries WHERE question LIKE ?1 ESCAPE '\\' OR sql LIKE ?1 ESCAPE '\\' OR tags LIKE ?1 ESCAPE '\\' OR note LIKE ?1 ESCAPE '\\' ORDER BY id DESC LIMIT 20")?;
            let rows = stmt.query_map([pattern], |row| {
                let tables: String = row.get(4)?;
                let tags: String = row.get(5)?;
                Ok(SavedQuery { id: row.get(0)?, source: row.get(1)?, question: row.get(2)?, sql: row.get(3)?, tables: serde_json::from_str(&tables).unwrap_or_default(), tags: serde_json::from_str(&tags).unwrap_or_default(), note: row.get(6)? })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        }).await?
    }

    pub async fn annotate(&self, id: i64, tags: Vec<String>, note: String) -> Result<()> {
        let Some(path) = self.path.clone() else {
            bail!("query memory is disabled")
        };
        if id <= 0 || !valid_annotation(&tags, &note) {
            bail!("invalid annotation")
        }
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&path)?;
            let count = conn.execute(
                "UPDATE queries SET tags=?1, note=?2 WHERE id=?3",
                params![serde_json::to_string(&tags)?, note, id],
            )?;
            if count != 1 {
                bail!("saved query not found")
            }
            Ok(())
        })
        .await??;
        Ok(())
    }

    pub async fn record_metadata(
        &self,
        provider: &str,
        key: &str,
        payload: &Value,
    ) -> Result<bool> {
        let Some(path) = self.path.clone() else {
            return Ok(false);
        };
        let payload = serde_json::to_string(payload)?;
        if payload.len() > 256_000 {
            bail!("metadata payload exceeds 256 KB")
        }
        let (provider, key) = (provider.to_owned(), key.to_owned());
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&path)?;
            conn.execute("INSERT INTO metadata (provider, entity_key, payload) VALUES (?1, ?2, ?3) ON CONFLICT(provider, entity_key) DO UPDATE SET payload=excluded.payload", params![provider, key, payload])?;
            Ok(())
        }).await??;
        Ok(true)
    }

    pub async fn search_metadata(&self, text: &str) -> Result<Vec<SavedMetadata>> {
        let Some(path) = self.path.clone() else {
            bail!("metadata memory is disabled")
        };
        let pattern = escape_like(&text.chars().take(200).collect::<String>());
        tokio::task::spawn_blocking(move || -> Result<Vec<SavedMetadata>> {
            let conn = open(&path)?;
            let mut stmt = conn.prepare("SELECT id, provider, entity_key, payload, tags, note FROM metadata WHERE entity_key LIKE ?1 ESCAPE '\\' OR payload LIKE ?1 ESCAPE '\\' OR tags LIKE ?1 ESCAPE '\\' OR note LIKE ?1 ESCAPE '\\' ORDER BY id DESC LIMIT 20")?;
            let rows = stmt.query_map([pattern], |row| {
                let payload: String = row.get(3)?;
                let tags: String = row.get(4)?;
                Ok(SavedMetadata { id: row.get(0)?, provider: row.get(1)?, key: row.get(2)?, payload: serde_json::from_str(&payload).unwrap_or(Value::Null), tags: serde_json::from_str(&tags).unwrap_or_default(), note: row.get(5)? })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
        }).await?
    }

    pub async fn annotate_metadata(&self, id: i64, tags: Vec<String>, note: String) -> Result<()> {
        let Some(path) = self.path.clone() else {
            bail!("metadata memory is disabled")
        };
        if id <= 0 || !valid_annotation(&tags, &note) {
            bail!("invalid annotation")
        }
        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = open(&path)?;
            let count = conn.execute(
                "UPDATE metadata SET tags=?1, note=?2 WHERE id=?3",
                params![serde_json::to_string(&tags)?, note, id],
            )?;
            if count != 1 {
                bail!("saved metadata not found")
            }
            Ok(())
        })
        .await??;
        Ok(())
    }

    pub async fn context(&self, text: &str) -> Result<Vec<ContextHit>> {
        let Some(root) = self.ontology_path.clone() else {
            bail!("ontology path is not configured")
        };
        let text = text.trim().to_lowercase();
        if text.len() < 2 || text.len() > 200 {
            bail!("context query must be 2-200 characters")
        }
        tokio::task::spawn_blocking(move || search_files(&root, &text)).await?
    }

    pub async fn graph(&self, graph: &str, node: &str, hops: u8) -> Result<Value> {
        let Some(root) = self.ontology_path.clone() else {
            bail!("ontology path is not configured")
        };
        if !matches!(
            graph,
            "joins" | "lineage" | "ontology" | "usage" | "mart_deps"
        ) || node.is_empty()
            || node.len() > 200
            || !(1..=2).contains(&hops)
        {
            bail!("invalid graph query")
        }
        let (graph, node) = (graph.to_owned(), node.to_owned());
        tokio::task::spawn_blocking(move || read_graph(&root, &graph, &node, hops)).await?
    }
}

fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path).context("could not open query memory")?;
    conn.busy_timeout(Duration::from_secs(3))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS queries (id INTEGER PRIMARY KEY, source TEXT NOT NULL, question TEXT NOT NULL, sql TEXT NOT NULL, tables TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '[]', note TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, UNIQUE(source, sql)); CREATE TABLE IF NOT EXISTS metadata (id INTEGER PRIMARY KEY, provider TEXT NOT NULL, entity_key TEXT NOT NULL, payload TEXT NOT NULL, tags TEXT NOT NULL DEFAULT '[]', note TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, UNIQUE(provider, entity_key));")?;
    Ok(conn)
}

fn escape_like(text: &str) -> String {
    format!(
        "%{}%",
        text.replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

fn valid_annotation(tags: &[String], note: &str) -> bool {
    tags.len() <= 12
        && note.len() <= 2000
        && tags.iter().all(|tag| {
            !tag.is_empty()
                && tag.len() <= 64
                && tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
}

fn search_files(root: &Path, needle: &str) -> Result<Vec<ContextHit>> {
    let root = root
        .canonicalize()
        .context("ontology root does not exist")?;
    let mut pending = vec![root.clone()];
    let mut hits = Vec::new();
    let mut seen = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            seen += 1;
            if seen > 5000 {
                bail!("ontology has too many files")
            }
            if entry.metadata()?.len() > 256_000 {
                continue;
            }
            let content = std::fs::read_to_string(&path)?;
            if let Some(line) = content
                .lines()
                .find(|line| line.to_lowercase().contains(needle))
            {
                hits.push(ContextHit {
                    path: path.strip_prefix(&root)?.display().to_string(),
                    excerpt: line.chars().take(300).collect(),
                });
                if hits.len() == 20 {
                    return Ok(hits);
                }
            }
        }
    }
    Ok(hits)
}

fn read_graph(root: &Path, graph: &str, node: &str, hops: u8) -> Result<Value> {
    let root = root
        .canonicalize()
        .context("ontology root does not exist")?;
    let path = root
        .join("graphs")
        .join(format!("{graph}.json"))
        .canonicalize()?;
    if !path.starts_with(&root) || std::fs::metadata(&path)?.len() > 4_000_000 {
        bail!("invalid ontology graph file")
    }
    let data: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let nodes = data
        .get("nodes")
        .and_then(Value::as_array)
        .context("graph has no nodes")?;
    let edges = data
        .get("edges")
        .and_then(Value::as_array)
        .context("graph has no edges")?;
    if nodes.len() > 20_000 || edges.len() > 100_000 {
        bail!("ontology graph is too large")
    }
    if !nodes
        .iter()
        .any(|item| item.get("id").and_then(Value::as_str) == Some(node))
    {
        bail!("ontology node not found")
    }
    let mut seen = HashSet::from([node.to_owned()]);
    let mut frontier = HashSet::from([node.to_owned()]);
    let mut selected = Vec::new();
    let mut truncated = false;
    for _ in 0..hops {
        let mut next = HashSet::new();
        for edge in edges {
            let (Some(from), Some(to)) = (
                edge.get("from").and_then(Value::as_str),
                edge.get("to").and_then(Value::as_str),
            ) else {
                continue;
            };
            if frontier.contains(from) || frontier.contains(to) {
                if selected.len() == 100 || seen.len() == 100 {
                    truncated = true;
                    break;
                }
                if !selected.contains(edge) {
                    selected.push(edge.clone());
                }
                for id in [from, to] {
                    if seen.insert(id.to_owned()) {
                        next.insert(id.to_owned());
                    }
                }
            }
        }
        if truncated || next.is_empty() {
            break;
        }
        frontier = next;
    }
    let nodes: Vec<_> = nodes
        .iter()
        .filter(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| seen.contains(id))
        })
        .cloned()
        .collect();
    Ok(json!({"graph": graph, "nodes": nodes, "edges": selected, "truncated": truncated}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn memory_is_opt_in_and_annotation_is_validated() {
        let disabled = MemoryStore::new(&Memory::default());
        assert!(!disabled.enabled());
        assert!(disabled.search("revenue").await.is_err());
    }

    #[tokio::test]
    async fn saves_and_annotates_successful_query() {
        let root = std::env::temp_dir().join(format!(
            "txttsql-memory-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("queries.sqlite");
        let store = MemoryStore::new(&Memory {
            enabled: true,
            path: Some(path),
            ontology_path: None,
        });
        store
            .record("db", "revenue", "SELECT 1", &[])
            .await
            .unwrap();
        let found = store.search("SELECT").await.unwrap();
        assert_eq!(found.len(), 1);
        store
            .annotate(found[0].id, vec!["verified".into()], "checked".into())
            .await
            .unwrap();
        assert_eq!(store.search("verified").await.unwrap()[0].note, "checked");
        assert!(
            store
                .record_metadata(
                    "airflow",
                    "payments",
                    &serde_json::json!({"dag_id":"payments", "owner":"data"})
                )
                .await
                .unwrap()
        );
        let entities = store.search_metadata("payments").await.unwrap();
        assert_eq!(entities[0].payload["owner"], "data");
        store
            .annotate_metadata(entities[0].id, vec!["critical".into()], "daily DAG".into())
            .await
            .unwrap();
        assert_eq!(
            store.search_metadata("critical").await.unwrap()[0].note,
            "daily DAG"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ontology_search_handles_unicode_case_mapping() {
        let root = std::env::temp_dir().join(format!("txttsql-ontology-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("term.md");
        std::fs::write(&file, "İstanbul metrics\nВыручка — сумма оплат\n").unwrap();
        let hits = search_files(&root, "выручка").unwrap();
        assert_eq!(hits[0].excerpt, "Выручка — сумма оплат");
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn graph_returns_typed_neighbors_without_following_outside_paths() {
        let root = std::env::temp_dir().join(format!("txttsql-graph-{}", std::process::id()));
        std::fs::create_dir_all(root.join("graphs")).unwrap();
        std::fs::write(root.join("graphs/ontology.json"), r#"{"nodes":[{"id":"payments","type":"domain"},{"id":"revenue","type":"metric"},{"id":"analytics.sales","type":"table"}],"edges":[{"from":"payments","to":"revenue","type":"belongs_to"},{"from":"revenue","to":"analytics.sales","type":"implements"}]}"#).unwrap();
        let one = read_graph(&root, "ontology", "payments", 1).unwrap();
        assert_eq!(one["edges"].as_array().unwrap().len(), 1);
        let two = read_graph(&root, "ontology", "payments", 2).unwrap();
        assert_eq!(two["edges"].as_array().unwrap().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
