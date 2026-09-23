use anyhow::{Result, bail};
use rmcp::{ServiceExt, transport::stdio};
use std::path::PathBuf;
use txttsql_mcp::McpServer;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut path = None;
    let mut env_file = None;
    while let Some(option) = args.next() {
        let value = args.next().ok_or_else(|| {
            anyhow::anyhow!("usage: txttsql-mcp [--config PATH] [--env-file PATH]")
        })?;
        match option.as_str() {
            "--config" if path.is_none() => path = Some(PathBuf::from(value)),
            "--env-file" if env_file.is_none() => env_file = Some(PathBuf::from(value)),
            _ => bail!("usage: txttsql-mcp [--config PATH] [--env-file PATH]"),
        }
    }
    if let Some(env_file) = env_file {
        dotenvy::from_path(env_file)?;
    }
    let path = path.unwrap_or_else(|| PathBuf::from("config.toml"));
    let server = McpServer::load(&path)?;
    server.serve(stdio()).await?.waiting().await?;
    Ok(())
}
