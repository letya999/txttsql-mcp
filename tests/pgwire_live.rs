//! Run against an explicitly created disposable PostgreSQL or CockroachDB.
use std::path::PathBuf;
use txttsql_mcp::{
    config::{DatabaseKind, SecretRef, Source},
    database::Registry,
    guard::{self, ApprovedQuery},
};

fn source(id: &str, table: &str) -> Source {
    let ca_file = std::env::var_os("PGWIRE_TEST_CA_FILE").map(PathBuf::from);
    let kind = match std::env::var("PGWIRE_TEST_KIND").unwrap().as_str() {
        "postgres" => DatabaseKind::Postgres,
        "cockroach" => DatabaseKind::Cockroach,
        _ => panic!("PGWIRE_TEST_KIND must be postgres or cockroach"),
    };
    Source {
        id: id.into(),
        kind,
        host: "127.0.0.1".into(),
        port: std::env::var("PGWIRE_TEST_PORT").unwrap().parse().unwrap(),
        database: "analytics".into(),
        user: std::env::var("PGWIRE_TEST_USER").unwrap(),
        user_secret: None,
        password: SecretRef::Env {
            name: "PGWIRE_TEST_PASSWORD".into(),
        },
        allow_insecure: ca_file.is_none(),
        ca_file,
        max_connections: 2,
        timeout_seconds: 5,
        max_rows: 2,
        allowed_schemas: vec![],
        allowed_tables: vec![table.into()],
    }
}

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL or CockroachDB and PGWIRE_TEST_* variables"]
async fn independent_sources_and_backend_read_only() {
    let first = source("first", "public.metrics");
    let second = source("second", "public.other");
    let registry = Registry::new(&[first.clone(), second.clone()]).unwrap();
    assert!(guard::validate(&first, "SELECT * FROM public.other", None).is_err());
    assert!(guard::validate(&second, "SELECT * FROM public.metrics", None).is_err());

    let query = guard::validate(&first, "SELECT id FROM public.metrics ORDER BY id", None).unwrap();
    let result = registry
        .get("first")
        .unwrap()
        .execute(&query)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 2);
    assert!(result.truncated);

    let query = guard::validate(&second, "SELECT id FROM public.other", None).unwrap();
    let result = registry
        .get("second")
        .unwrap()
        .execute(&query)
        .await
        .unwrap();
    assert_eq!(result.rows.len(), 1);

    // Bypass the SQL guard here to prove the adapter's database-side read-only boundary.
    let forbidden = ApprovedQuery {
        sql: "SELECT nextval('public.testseq') AS n".into(),
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
    let query = guard::validate(&first, "SELECT COUNT(*) AS n FROM public.metrics", None).unwrap();
    let result = registry
        .get("first")
        .unwrap()
        .execute(&query)
        .await
        .unwrap();
    assert_eq!(result.rows[0]["n"], 3);
}
