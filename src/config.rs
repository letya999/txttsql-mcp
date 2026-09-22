use anyhow::{Context, Result, bail};
use secrecy::SecretString;
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, time::timeout};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub sources: Vec<Source>,
    #[serde(default)]
    pub memory: Memory,
    pub openmetadata: Option<ApiSource>,
    pub airflow: Option<ApiSource>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub kind: DatabaseKind,
    pub host: String,
    pub port: u16,
    pub database: String,
    #[serde(default)]
    pub user: String,
    pub user_secret: Option<SecretRef>,
    pub password: SecretRef,
    #[serde(default)]
    pub allow_insecure: bool,
    pub ca_file: Option<PathBuf>,
    #[serde(default = "default_pool_size")]
    pub max_connections: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_limit")]
    pub max_rows: u32,
    #[serde(default)]
    pub allowed_schemas: Vec<String>,
    #[serde(default)]
    pub allowed_tables: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseKind {
    Postgres,
    Cockroach,
    Clickhouse,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SecretRef {
    Env {
        name: String,
    },
    File {
        path: PathBuf,
    },
    Broker {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSource {
    pub url: String,
    pub token: Option<SecretRef>,
    pub basic_user: Option<String>,
    pub basic_password: Option<SecretRef>,
    #[serde(default)]
    pub allow_insecure: bool,
    pub ca_file: Option<PathBuf>,
    /// Airflow 2 uses v1; Airflow 3 uses v2. Ignored for OpenMetadata.
    pub api_version: Option<u8>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    #[serde(default)]
    pub enabled: bool,
    pub path: Option<PathBuf>,
    pub ontology_path: Option<PathBuf>,
}

fn default_pool_size() -> u32 {
    8
}
fn default_timeout() -> u64 {
    30
}
fn default_limit() -> u32 {
    1000
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read config {}", path.display()))?;
        let config: Self = toml::from_str(&raw).context("invalid configuration")?;
        if config.sources.is_empty() {
            bail!("at least one source is required")
        }
        let mut ids = HashSet::new();
        for source in &config.sources {
            if !valid_id(&source.id) || !ids.insert(&source.id) {
                bail!("invalid or duplicate source id")
            }
            if source.host.is_empty()
                || source.host.len() > 255
                || source.database.is_empty()
                || source.database.len() > 128
                || (source.user.is_empty() && source.user_secret.is_none())
                || (!source.user.is_empty() && source.user_secret.is_some())
                || source.user.len() > 256
                || source.port == 0
            {
                bail!("source {} has missing connection fields", source.id)
            }
            if source.max_connections == 0
                || source.max_connections > 128
                || source.max_rows == 0
                || source.max_rows > 100_000
                || source.timeout_seconds == 0
                || source.timeout_seconds > 300
                || source.allowed_schemas.len() > 100
                || source.allowed_tables.len() > 1000
            {
                bail!("source {} has invalid limits", source.id)
            }
            if source.allow_insecure && source.ca_file.is_some() {
                bail!(
                    "source {} cannot combine allow_insecure with ca_file",
                    source.id
                )
            }
            for name in &source.allowed_schemas {
                if !valid_object_name(name) || name.contains('.') {
                    bail!("source {} has invalid schema allowlist entry", source.id)
                }
            }
            for name in &source.allowed_tables {
                if !valid_object_name(name) || name.split('.').count() != 2 {
                    bail!("source {} has invalid table allowlist entry", source.id)
                }
            }
        }
        if config.memory.enabled && config.memory.path.is_none() {
            bail!("memory.path is required when memory is enabled")
        }
        if config
            .airflow
            .as_ref()
            .and_then(|api| api.api_version)
            .is_some_and(|version| version != 1 && version != 2)
        {
            bail!("Airflow api_version must be 1 or 2")
        }
        for api in [&config.openmetadata, &config.airflow]
            .into_iter()
            .flatten()
        {
            if api.allow_insecure && api.ca_file.is_some() {
                bail!("metadata source cannot combine allow_insecure with ca_file")
            }
            if (api.token.is_some() && (api.basic_user.is_some() || api.basic_password.is_some()))
                || (api.token.is_none()
                    && (api.basic_user.as_ref().is_none_or(String::is_empty)
                        || api.basic_password.is_none()))
                || api
                    .basic_user
                    .as_ref()
                    .is_some_and(|user| user.len() > 256 || user.chars().any(char::is_control))
            {
                bail!("metadata source requires either bearer token or basic credentials")
            }
            let url = reqwest::Url::parse(&api.url).context("invalid metadata URL")?;
            if !matches!(url.scheme(), "https" | "http")
                || (url.scheme() != "https" && !api.allow_insecure)
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
            {
                bail!(
                    "metadata URL must be a plain origin using HTTPS unless allow_insecure is set"
                )
            }
        }
        Ok(config)
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_object_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('.').all(|part| {
            part.len() <= 128
                && part
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

pub fn add_ca_file(
    mut builder: reqwest::ClientBuilder,
    path: Option<&Path>,
) -> Result<reqwest::ClientBuilder> {
    if let Some(path) = path {
        let pem = std::fs::read(path).context("cannot read CA file")?;
        let certificates =
            reqwest::Certificate::from_pem_bundle(&pem).context("invalid CA certificate bundle")?;
        if certificates.is_empty() {
            bail!("CA certificate bundle is empty")
        }
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    Ok(builder)
}

impl SecretRef {
    pub async fn resolve(&self) -> Result<SecretString> {
        let value = match self {
            Self::Env { name } => {
                std::env::var(name).with_context(|| format!("secret env var {name} is missing"))?
            }
            Self::File { path } => {
                let file = tokio::fs::File::open(path)
                    .await
                    .with_context(|| format!("cannot read secret file {}", path.display()))?;
                let mut bytes = Vec::new();
                file.take(8193).read_to_end(&mut bytes).await?;
                String::from_utf8(bytes).context("secret file is not UTF-8")?
            }
            Self::Broker { program, args } => {
                if cfg!(windows)
                    && program.extension().is_some_and(|extension| {
                        matches!(
                            extension.to_string_lossy().to_ascii_lowercase().as_str(),
                            "bat" | "cmd"
                        )
                    })
                {
                    bail!("Windows credential broker must be an executable, not a batch script")
                }
                let mut child = Command::new(program)
                    .args(args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .context("credential broker failed to start")?;
                let stdout = child
                    .stdout
                    .take()
                    .context("credential broker has no stdout")?;
                let bytes = timeout(Duration::from_secs(5), async {
                    let mut bytes = Vec::new();
                    stdout.take(8193).read_to_end(&mut bytes).await?;
                    if bytes.len() > 8192 || !child.wait().await?.success() {
                        bail!("credential broker failed")
                    }
                    Ok::<_, anyhow::Error>(bytes)
                })
                .await
                .context("credential broker timed out")??;
                String::from_utf8(bytes).context("credential broker returned non UTF-8")?
            }
        };
        let value = value.trim_end_matches(['\r', '\n']);
        if value.is_empty() || value.len() > 8192 {
            bail!("resolved credential is empty or too large")
        }
        Ok(SecretString::new(value.to_owned().into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ids_and_allowlist_are_strict() {
        assert!(valid_id("prod-ch_1"));
        assert!(!valid_id("prod/ch"));
        assert!(valid_object_name("analytics.payments"));
        assert!(!valid_object_name("analytics.*"));
    }

    #[test]
    fn metadata_origins_and_airflow_versions_are_checked() {
        let root = std::env::temp_dir().join(format!("txttsql-config-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        let source = "[[sources]]\nid='db'\nkind='postgres'\nhost='localhost'\nport=5432\ndatabase='db'\nuser_secret={kind='env',name='DB_USER'}\npassword={kind='env',name='DB_PASSWORD'}\nallowed_schemas=['public']\n";
        std::fs::write(&path, format!("{source}\n[airflow]\nurl='https://user:pass@example.com'\ntoken={{kind='env',name='TOKEN'}}\n")).unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(&path, format!("{source}\n[airflow]\nurl='https://example.com'\ntoken={{kind='env',name='TOKEN'}}\napi_version=3\n")).unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(&path, format!("{source}\n[airflow]\nurl='https://example.com'\ntoken={{kind='env',name='TOKEN'}}\napi_version=2\n")).unwrap();
        assert!(Config::load(&path).is_ok());
        std::fs::write(&path, format!("{source}\n[airflow]\nurl='https://example.com'\ntoken={{kind='env',name='TOKEN'}}\nallow_insecure=true\nca_file='internal.pem'\n")).unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(
            &path,
            source.replace(
                "allowed_schemas=['public']",
                "allowed_schemas=['public']\nallow_insecure=true\nca_file='internal.pem'",
            ),
        )
        .unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(&path, format!("{source}\n[airflow]\nurl='https://example.com'\ntoken={{kind='env',name='TOKEN'}}\nbasic_user='reader'\nbasic_password={{kind='env',name='PASSWORD'}}\n")).unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::write(
            &path,
            source.replace("allowed_schemas=['public']", "allowed_tables=['t']"),
        )
        .unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[tokio::test]
    async fn secret_file_is_trimmed_and_bounded() {
        use secrecy::ExposeSecret;
        let path = std::env::temp_dir().join(format!("txttsql-secret-{}", std::process::id()));
        std::fs::write(&path, "test-credential\r\n").unwrap();
        let secret = SecretRef::File { path: path.clone() };
        assert_eq!(
            secret.resolve().await.unwrap().expose_secret(),
            "test-credential"
        );
        std::fs::write(&path, "x".repeat(8193)).unwrap();
        assert!(secret.resolve().await.is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_ca_bundle_is_rejected() {
        let path = std::env::temp_dir().join(format!("txttsql-ca-{}", std::process::id()));
        std::fs::write(&path, "not a PEM certificate").unwrap();
        assert!(add_ca_file(reqwest::Client::builder(), Some(&path)).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_broker_rejects_batch_scripts() {
        let broker = SecretRef::Broker {
            program: "broker.cmd".into(),
            args: vec![],
        };
        assert!(broker.resolve().await.is_err());
    }
}
