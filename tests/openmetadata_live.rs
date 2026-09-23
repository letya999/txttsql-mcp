//! Run only against an explicitly selected test OpenMetadata instance.
use txttsql_mcp::{
    config::{ApiSource, Config, SecretRef},
    metadata::{MetadataAdapter, Registry},
    plugin::Registry as PluginRegistry,
};

#[tokio::test]
#[ignore = "requires OPENMETADATA_TEST_URL, OPENMETADATA_TEST_TOKEN and OPENMETADATA_TEST_TABLE"]
async fn searches_and_describes_real_table() {
    let source = ApiSource {
        url: std::env::var("OPENMETADATA_TEST_URL").unwrap(),
        token: Some(SecretRef::Env {
            name: "OPENMETADATA_TEST_TOKEN".into(),
        }),
        basic_user: None,
        basic_password: None,
        allow_insecure: false,
        ca_file: None,
        api_version: None,
    };
    let table = std::env::var("OPENMETADATA_TEST_TABLE").unwrap();
    let registry = Registry::new(Some(source), None).unwrap();
    let metadata = registry.get("openmetadata").unwrap();
    assert!(metadata.search(&table).await.unwrap()["hits"].is_object());
    let detail = metadata.detail(&table).await.unwrap();
    assert!(detail["name"].is_string());
    assert!(detail["columns"].is_array());
}

#[tokio::test]
#[ignore = "requires disposable OpenMetadata with OPENMETADATA_TEST_URL, OPENMETADATA_TEST_TOKEN and OPENMETADATA_TEST_TABLE"]
async fn plugin_searches_and_describes_real_table() {
    let url = std::env::var("OPENMETADATA_TEST_URL").unwrap();
    let table = std::env::var("OPENMETADATA_TEST_TABLE").unwrap();
    let path = std::env::temp_dir().join(format!(
        "txttsql-openmetadata-plugin-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/openmetadata/plugin.json");
    let config = format!(
        "[[plugin_metadata]]\nid='openmetadata_live'\nmanifest={}\n[plugin_metadata.settings]\nurl={}\nallow_insecure={}\n[plugin_metadata.secrets]\ntoken={{kind='env',name='OPENMETADATA_TEST_TOKEN'}}\n",
        toml::Value::String(manifest.to_string_lossy().into_owned()),
        toml::Value::String(url.clone()),
        url.starts_with("http://127.0.0.1:")
    );
    std::fs::write(&path, config).unwrap();
    let config = Config::load(&path).unwrap();
    let plugins = PluginRegistry::new(&config, path.parent().unwrap()).unwrap();
    let plugin = plugins.metadata().next().unwrap();
    assert!(plugin.search(&table).await.unwrap()["hits"].is_object());
    let detail = plugin.detail(&table).await.unwrap();
    assert!(detail["name"].is_string());
    assert!(detail["columns"].is_array());
    std::fs::remove_file(path).unwrap();
}
