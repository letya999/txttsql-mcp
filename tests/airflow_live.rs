//! Run only against an explicitly created disposable Airflow 2 instance.
use txttsql_mcp::{
    config::{ApiSource, SecretRef},
    metadata::Registry,
};

#[tokio::test]
#[ignore = "requires disposable Airflow 2 with AIRFLOW_TEST_URL, AIRFLOW_TEST_USER, AIRFLOW_TEST_PASSWORD, AIRFLOW_TEST_DAG"]
async fn discovers_and_describes_real_dag() {
    let source = ApiSource {
        url: std::env::var("AIRFLOW_TEST_URL").unwrap(),
        token: None,
        basic_user: Some(std::env::var("AIRFLOW_TEST_USER").unwrap()),
        basic_password: Some(SecretRef::Env {
            name: "AIRFLOW_TEST_PASSWORD".into(),
        }),
        allow_insecure: true,
        ca_file: None,
        api_version: Some(1),
    };
    let dag_id = std::env::var("AIRFLOW_TEST_DAG").unwrap();
    let registry = Registry::new(None, Some(source)).unwrap();
    let airflow = registry.get("airflow").unwrap();
    let found = airflow.search(&dag_id).await.unwrap();
    assert!(
        found["dags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|dag| dag["dag_id"] == dag_id)
    );
    let detail = airflow.detail(&dag_id).await.unwrap();
    assert_eq!(detail["dag"]["dag_id"], dag_id);
    assert!(!detail["tasks"].as_array().unwrap().is_empty());
    assert!(!detail["edges"].as_array().unwrap().is_empty());
}
