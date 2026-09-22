//! Run against an explicitly created disposable ClickHouse, never a production database.
use txttsql_mcp::{
    config::{DatabaseKind, SecretRef, Source},
    database::Registry,
    guard::{self, ApprovedQuery},
};

fn source(id: &str, table: &str) -> Source {
    Source {
        id: id.into(),
        kind: DatabaseKind::Clickhouse,
        host: "127.0.0.1".into(),
        port: std::env::var("CH_TEST_PORT").unwrap().parse().unwrap(),
        database: "analytics".into(),
        user: "default".into(),
        user_secret: None,
        password: SecretRef::Env {
            name: "CH_TEST_PASSWORD".into(),
        },
        allow_insecure: true,
        max_connections: 2,
        timeout_seconds: 5,
        max_rows: 2,
        allowed_schemas: vec![],
        allowed_tables: vec![table.into()],
    }
}

#[tokio::test]
#[ignore = "requires a disposable ClickHouse with CH_TEST_PORT and CH_TEST_PASSWORD"]
async fn independent_sources_and_backend_read_only() {
    let first = source("first", "analytics.metrics");
    let second = source("second", "analytics.other");
    let registry = Registry::new(&[first.clone(), second.clone()]).unwrap();
    assert!(guard::validate(&first, "SELECT * FROM analytics.other", None).is_err());
    assert!(guard::validate(&second, "SELECT * FROM analytics.metrics", None).is_err());

    let query =
        guard::validate(&first, "SELECT id FROM analytics.metrics ORDER BY id", None).unwrap();
    let result = registry
        .get("first")
        .unwrap()
        .execute(&query)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 2);
    assert!(result.truncated);

    let query = guard::validate(&second, "SELECT id FROM analytics.other", None).unwrap();
    let result = registry
        .get("second")
        .unwrap()
        .execute(&query)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 1);

    // Bypass the SQL guard to prove the HTTP query's database-side readonly setting.
    let forbidden = ApprovedQuery {
        sql: "DROP TABLE analytics.other".into(),
        tables: vec![],
        row_cap: 2,
    };
    assert!(
        registry
            .get("first")
            .unwrap()
            .execute(&forbidden)
            .await
            .is_err()
    );
    let query = guard::validate(&second, "SELECT count() AS n FROM analytics.other", None).unwrap();
    let result = registry
        .get("second")
        .unwrap()
        .execute(&query)
        .await
        .unwrap();
    assert_eq!(result.rows[0]["n"], 1);
}
