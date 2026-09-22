//! Run only against an explicitly selected test OpenMetadata instance.
use txttsql_mcp::{
    config::{ApiSource, SecretRef},
    metadata::Registry,
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
