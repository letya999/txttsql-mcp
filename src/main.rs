use anyhow::{Result, bail};
use rmcp::{ServiceExt, transport::stdio};
use std::path::PathBuf;
use txttsql_mcp::McpServer;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = match (args.next().as_deref(), args.next()) {
        (None, None) => PathBuf::from("config.toml"),
        (Some("--config"), Some(path)) => PathBuf::from(path),
        _ => bail!("usage: txttsql-mcp [--config PATH]"),
    };
    let server = McpServer::load(&path)?;
    server.serve(stdio()).await?.waiting().await?;
    Ok(())
}
