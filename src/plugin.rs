//! Versioned, out-of-process adapters loaded from explicitly configured packages.
use crate::{
    config::{PluginDatabase, SecretRef, valid_id},
    database::{DatabaseAdapter, QueryResult},
    guard::{ApprovedQuery, SqlDialect, SqlPolicy, table_allowed_policy},
    metadata::MetadataAdapter,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
    time::timeout,
};

const PROTOCOL: u32 = 1;
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Database,
    Metadata,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::Metadata => "metadata",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    manifest_version: u32,
    id: String,
    version: String,
    kind: Kind,
    dialect: Option<SqlDialect>,
    command: String,
    command_windows: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    capabilities: Vec<String>,
}

struct Manifest {
    id: String,
    kind: Kind,
    dialect: Option<SqlDialect>,
    capabilities: HashSet<String>,
    program: PathBuf,
    args: Vec<String>,
    directory: PathBuf,
}

impl Manifest {
    fn load(config_dir: &Path, path: &Path, expected: Kind) -> Result<Self> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            config_dir.join(path)
        };
        if std::fs::metadata(&path)?.len() > 16_384 {
            bail!("plugin manifest is too large")
        }
        let file: ManifestFile =
            serde_json::from_slice(&std::fs::read(&path)?).context("invalid plugin manifest")?;
        if file.manifest_version != PROTOCOL
            || file.kind != expected
            || !valid_id(&file.id)
            || file.version.is_empty()
            || file.version.len() > 64
            || file.command.is_empty()
            || file.command.len() > 512
            || file
                .command_windows
                .as_ref()
                .is_some_and(|command| command.is_empty() || command.len() > 512)
            || file.args.len() > 16
            || file.args.iter().any(|arg| arg.len() > 512)
            || (file.kind == Kind::Database) != file.dialect.is_some()
        {
            bail!("unsupported or invalid plugin manifest")
        }
        let required: &[&str] = match file.kind {
            Kind::Database => &["execute", "list_tables", "describe_table"],
            Kind::Metadata => &["search", "detail"],
        };
        let capabilities: HashSet<_> = file.capabilities.iter().map(String::as_str).collect();
        if capabilities.len() != file.capabilities.len()
            || !required.iter().all(|method| capabilities.contains(method))
        {
            bail!("plugin is missing required capabilities")
        }
        let directory = path
            .canonicalize()?
            .parent()
            .context("plugin manifest has no directory")?
            .to_owned();
        let command = Path::new(if cfg!(windows) {
            file.command_windows.as_deref().unwrap_or(&file.command)
        } else {
            &file.command
        });
        let program = if command.is_absolute() || command.components().count() == 1 {
            command.to_owned()
        } else {
            let program = directory.join(command).canonicalize()?;
            if !program.starts_with(&directory) {
                bail!("plugin command leaves its package directory")
            }
            program
        };
        Ok(Self {
            id: file.id,
            kind: file.kind,
            dialect: file.dialect,
            capabilities: file.capabilities.into_iter().collect(),
            program,
            args: file.args,
            directory,
        })
    }
}

#[derive(Deserialize)]
struct Response {
    protocol: u32,
    id: u64,
    ok: bool,
    #[serde(default)]
    result: Value,
    error: Option<String>,
}

struct Session {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Session {
    async fn start(manifest: &Manifest) -> Result<Self> {
        let mut command = Command::new(&manifest.program);
        command
            .args(&manifest.args)
            .current_dir(&manifest.directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for name in ["PATH", "SystemRoot"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn().context("plugin could not start")?;
        let stdin = child.stdin.take().context("plugin stdin unavailable")?;
        let stdout = BufReader::new(child.stdout.take().context("plugin stdout unavailable")?);
        let mut session = Self {
            _child: child,
            stdin,
            stdout,
            next_id: 0,
        };
        let response = session
            .exchange(
                "hello",
                json!({"plugin": manifest.id, "kind": manifest.kind.as_str()}),
            )
            .await?;
        if !response.ok
            || response.result["plugin"] != manifest.id
            || response.result["kind"] != manifest.kind.as_str()
            || response.result["capabilities"]
                .as_array()
                .is_none_or(|items| {
                    let declared: HashSet<_> = items.iter().filter_map(Value::as_str).collect();
                    declared.len() != items.len()
                        || declared != manifest.capabilities.iter().map(String::as_str).collect()
                })
        {
            bail!("plugin handshake did not match its manifest")
        }
        Ok(session)
    }

    async fn exchange(&mut self, method: &str, payload: Value) -> Result<Response> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = serde_json::to_vec(&json!({
            "protocol": PROTOCOL,
            "id": id,
            "method": method,
            "payload": payload,
        }))?;
        if request.len() > 256_000 {
            bail!("plugin request is too large")
        }
        request.push(b'\n');
        self.stdin.write_all(&request).await?;
        self.stdin.flush().await?;
        let response: Response = serde_json::from_slice(&read_line(&mut self.stdout).await?)
            .context("invalid plugin response")?;
        if response.protocol != PROTOCOL || response.id != id {
            bail!("plugin protocol mismatch")
        }
        Ok(response)
    }
}

async fn read_line(reader: &mut BufReader<ChildStdout>) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            bail!("plugin closed stdout")
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let count = end.map_or(available.len(), |index| index + 1);
        if line.len() + count > MAX_MESSAGE {
            bail!("plugin response exceeds 16 MB")
        }
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        if end.is_some() {
            return Ok(line);
        }
    }
}

struct Host {
    manifest: Manifest,
    instance: String,
    settings: Value,
    secrets: HashMap<String, SecretRef>,
    workers: Vec<Mutex<Option<Session>>>,
    next_worker: AtomicUsize,
    timeout: Duration,
}

impl Host {
    fn new(
        manifest: Manifest,
        instance: String,
        settings: &toml::Table,
        secrets: HashMap<String, SecretRef>,
        workers: u32,
        timeout_seconds: u64,
    ) -> Result<Self> {
        Ok(Self {
            manifest,
            instance,
            settings: serde_json::to_value(settings)?,
            secrets,
            workers: (0..workers).map(|_| Mutex::new(None)).collect(),
            next_worker: AtomicUsize::new(0),
            timeout: Duration::from_secs(timeout_seconds + 5),
        })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut secrets = serde_json::Map::new();
        for (name, reference) in &self.secrets {
            secrets.insert(
                name.clone(),
                Value::String(reference.resolve().await?.expose_secret().to_owned()),
            );
        }
        let payload = json!({
            "instance": self.instance,
            "settings": self.settings,
            "secrets": secrets,
            "params": params,
        });
        let index = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        let mut slot = timeout(self.timeout, self.workers[index].lock())
            .await
            .context("plugin worker queue timed out")?;
        let result = timeout(self.timeout, async {
            if slot.is_none() {
                *slot = Some(Session::start(&self.manifest).await?);
            }
            slot.as_mut().unwrap().exchange(method, payload).await
        })
        .await;
        let response = match result {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                *slot = None;
                return Err(err);
            }
            Err(_) => {
                *slot = None;
                bail!("plugin request timed out")
            }
        };
        if !response.ok {
            bail!(
                "plugin rejected request: {}",
                response.error.as_deref().unwrap_or("unknown")
            )
        }
        Ok(response.result)
    }
}

pub struct DatabasePlugin {
    pub config: PluginDatabase,
    host: Host,
    dialect: SqlDialect,
}

impl DatabasePlugin {
    pub fn plugin_id(&self) -> &str {
        &self.host.manifest.id
    }

    pub fn policy(&self) -> SqlPolicy<'_> {
        SqlPolicy {
            dialect: self.dialect,
            max_rows: self.config.max_rows,
            allowed_schemas: &self.config.allowed_schemas,
            allowed_tables: &self.config.allowed_tables,
        }
    }

    pub async fn list_tables(&self) -> Result<Vec<String>> {
        let result = self.host.call("list_tables", json!({})).await?;
        let tables: Vec<String> = serde_json::from_value(result["tables"].clone())?;
        if tables.len() > 1000 {
            bail!("plugin returned too many tables")
        }
        Ok(tables
            .into_iter()
            .filter(|name| table_allowed_policy(&self.policy(), name))
            .collect())
    }

    pub async fn describe_table(&self, table: &str) -> Result<Value> {
        if !table_allowed_policy(&self.policy(), table) {
            bail!("table is outside the source allowlist")
        }
        let result = self
            .host
            .call("describe_table", json!({"table": table}))
            .await?;
        let columns = result["columns"]
            .as_array()
            .context("plugin returned no columns")?;
        if columns.len() > 1000
            || columns.iter().any(|column| {
                column["column_name"]
                    .as_str()
                    .is_none_or(|name| name.len() > 128)
                    || column["data_type"]
                        .as_str()
                        .is_none_or(|name| name.len() > 200)
            })
        {
            bail!("plugin returned invalid columns")
        }
        Ok(result)
    }
}

#[async_trait]
impl DatabaseAdapter for DatabasePlugin {
    async fn execute(&self, query: &ApprovedQuery) -> Result<QueryResult> {
        let result = self
            .host
            .call(
                "execute",
                json!({"sql": query.sql, "row_cap": query.row_cap, "tables": query.tables}),
            )
            .await?;
        let result: QueryResult = serde_json::from_value(result)?;
        if result.rows.len() > query.row_cap as usize
            || serde_json::to_vec(&result)?.len() > MAX_MESSAGE
        {
            bail!("plugin query result exceeds source limits")
        }
        Ok(result)
    }
}

pub struct MetadataPlugin {
    pub id: String,
    host: Host,
}

#[async_trait]
impl MetadataAdapter for MetadataPlugin {
    async fn search(&self, text: &str) -> Result<Value> {
        if text.len() > 200 {
            bail!("search text is too long")
        }
        let result = self.host.call("search", json!({"text": text})).await?;
        if serde_json::to_vec(&result)?.len() > 4 * 1024 * 1024 {
            bail!("metadata plugin response exceeds 4 MB")
        }
        Ok(result)
    }

    async fn detail(&self, id: &str) -> Result<Value> {
        if id.is_empty() || id.len() > 300 {
            bail!("invalid metadata id")
        }
        let result = self.host.call("detail", json!({"id": id})).await?;
        if serde_json::to_vec(&result)?.len() > 4 * 1024 * 1024 {
            bail!("metadata plugin response exceeds 4 MB")
        }
        Ok(result)
    }
}

pub struct Registry {
    databases: HashMap<String, Arc<DatabasePlugin>>,
    metadata: HashMap<String, Arc<MetadataPlugin>>,
}

impl Registry {
    pub fn new(config: &crate::config::Config, config_dir: &Path) -> Result<Self> {
        let mut databases = HashMap::new();
        for source in &config.plugin_databases {
            let manifest = Manifest::load(config_dir, &source.manifest, Kind::Database)?;
            let dialect = manifest.dialect.context("database plugin has no dialect")?;
            let host = Host::new(
                manifest,
                source.id.clone(),
                &source.settings,
                source.secrets.clone(),
                source.workers,
                source.timeout_seconds,
            )?;
            databases.insert(
                source.id.clone(),
                Arc::new(DatabasePlugin {
                    config: source.clone(),
                    host,
                    dialect,
                }),
            );
        }
        let mut metadata = HashMap::new();
        for provider in &config.plugin_metadata {
            let manifest = Manifest::load(config_dir, &provider.manifest, Kind::Metadata)?;
            let host = Host::new(
                manifest,
                provider.id.clone(),
                &provider.settings,
                provider.secrets.clone(),
                1,
                provider.timeout_seconds,
            )?;
            metadata.insert(
                provider.id.clone(),
                Arc::new(MetadataPlugin {
                    id: provider.id.clone(),
                    host,
                }),
            );
        }
        Ok(Self {
            databases,
            metadata,
        })
    }

    pub fn database(&self, id: &str) -> Option<Arc<DatabasePlugin>> {
        self.databases.get(id).cloned()
    }

    pub fn databases(&self) -> impl Iterator<Item = Arc<DatabasePlugin>> + '_ {
        self.databases.values().cloned()
    }

    pub fn metadata(&self) -> impl Iterator<Item = Arc<MetadataPlugin>> + '_ {
        self.metadata.values().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rejects_wrong_kind_version_and_capabilities() {
        let path = std::env::temp_dir().join(format!(
            "txttsql-plugin-manifest-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut manifest = json!({
            "manifest_version": 1,
            "id": "example",
            "version": "1.0.0",
            "kind": "database",
            "dialect": "ansi",
            "command": "python3",
            "capabilities": ["execute", "list_tables", "describe_table"]
        });
        let write = |manifest: &Value| {
            std::fs::write(&path, serde_json::to_vec(manifest).unwrap()).unwrap()
        };
        write(&manifest);
        assert!(Manifest::load(Path::new("."), &path, Kind::Database).is_ok());
        assert!(Manifest::load(Path::new("."), &path, Kind::Metadata).is_err());
        manifest["manifest_version"] = json!(2);
        write(&manifest);
        assert!(Manifest::load(Path::new("."), &path, Kind::Database).is_err());
        manifest["manifest_version"] = json!(1);
        manifest["capabilities"] = json!(["execute"]);
        write(&manifest);
        assert!(Manifest::load(Path::new("."), &path, Kind::Database).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
