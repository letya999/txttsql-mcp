use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    process::{ChildStdin, ChildStdout, Command, Stdio},
};

fn send(input: &mut ChildStdin, output: &mut BufReader<ChildStdout>, message: Value) -> Value {
    writeln!(input, "{message}").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert!(!line.is_empty(), "MCP server exited early");
    serde_json::from_str::<Value>(&line).unwrap()
}

#[test]
#[ignore = "requires disposable PostgreSQL, CockroachDB and ClickHouse with MIXED_TEST_* variables"]
fn one_mcp_process_serves_three_databases() {
    let (pg, cr, ch) = (
        std::env::var("MIXED_TEST_PG_PORT").unwrap(),
        std::env::var("MIXED_TEST_CR_PORT").unwrap(),
        std::env::var("MIXED_TEST_CH_PORT").unwrap(),
    );
    let config = format!(
        "[[sources]]\nid='pg'\nkind='postgres'\nhost='127.0.0.1'\nport={pg}\ndatabase='analytics'\nuser='postgres'\npassword={{kind='env',name='MIXED_TEST_PASSWORD'}}\nallowed_tables=['public.metrics']\nallow_insecure=true\nmax_rows=2\n\n[[sources]]\nid='cr'\nkind='cockroach'\nhost='127.0.0.1'\nport={cr}\ndatabase='analytics'\nuser='root'\npassword={{kind='env',name='MIXED_TEST_PASSWORD'}}\nallowed_tables=['public.metrics']\nallow_insecure=true\nmax_rows=2\n\n[[sources]]\nid='ch'\nkind='clickhouse'\nhost='127.0.0.1'\nport={ch}\ndatabase='analytics'\nuser='default'\npassword={{kind='env',name='MIXED_TEST_PASSWORD'}}\nallowed_tables=['analytics.metrics']\nallow_insecure=true\nmax_rows=2\n"
    );
    let config = config
        .replace(
            "allowed_tables=['public.metrics']",
            "allowed_tables=['public.metrics']\nallowed_schemas=['public']",
        )
        .replace(
            "allowed_tables=['analytics.metrics']",
            "allowed_tables=['analytics.metrics']\nallowed_schemas=['analytics']",
        );
    let path = std::env::temp_dir().join(format!("txttsql-mixed-{}.toml", std::process::id()));
    std::fs::write(&path, config).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_txttsql-mcp"))
        .arg("--config")
        .arg(&path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let init = send(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"mixed-test","version":"1"}}}),
    );
    assert!(init.get("result").is_some(), "{init}");
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    for (id, source, sql) in [
        (2, "pg", "SELECT id FROM public.metrics ORDER BY id"),
        (3, "cr", "SELECT id FROM public.metrics ORDER BY id"),
        (4, "ch", "SELECT id FROM analytics.metrics ORDER BY id"),
    ] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"execute_sql","arguments":{"source":source,"sql":sql}}})).unwrap();
    }
    input.flush().unwrap();
    let mut results = HashMap::new();
    for _ in 0..3 {
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        let payload: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        results.insert(response["id"].as_i64().unwrap(), payload);
    }
    for id in 2..=4 {
        assert_eq!(results[&id]["ok"], true, "{}", results[&id]);
        assert_eq!(results[&id]["result"]["rows"].as_array().unwrap().len(), 2);
        assert_eq!(results[&id]["result"]["truncated"], true);
    }
    for (id, source, table) in [
        (5, "pg", "public.metrics"),
        (7, "cr", "public.metrics"),
        (9, "ch", "analytics.metrics"),
    ] {
        let response = send(
            &mut input,
            &mut output,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"describe_table","arguments":{"source":source,"table":table}}}),
        );
        let payload: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(payload["ok"], true, "{payload}");
        assert_eq!(payload["columns"][0]["column_name"], "id");
        let response = send(
            &mut input,
            &mut output,
            json!({"jsonrpc":"2.0","id":id+1,"method":"tools/call","params":{"name":"list_tables","arguments":{"source":source}}}),
        );
        let payload: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(payload["ok"], true, "{payload}");
        assert!(!payload["discovered"].as_array().unwrap().is_empty());
    }
    drop(input);
    child.kill().ok();
    child.wait().unwrap();
    std::fs::remove_file(path).unwrap();
}

#[test]
fn stdio_initializes_lists_tools_and_validates() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_txttsql-mcp"))
        .args([
            "--config",
            concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.toml"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let initialize = send(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}),
    );
    assert!(initialize.get("result").is_some(), "{initialize}");
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    input.flush().unwrap();

    let tools = send(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
    );
    assert!(
        tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "execute_sql")
    );

    let result = send(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"validate_sql","arguments":{"source":"app_pg","sql":"SELECT id FROM public.users LIMIT 99999","max_rows":5}}}),
    );
    let payload: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["ok"], true, "{payload}");
    assert!(payload["sql"].as_str().unwrap().ends_with("LIMIT 6"));
    drop(input);
    child.kill().ok();
    child.wait().unwrap();
}

#[test]
fn loads_independent_database_and_metadata_plugins() {
    let root = std::env::temp_dir().join(format!(
        "txttsql-plugin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let database = root.join("metrics.sqlite");
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute_batch("CREATE TABLE metrics(id INTEGER); INSERT INTO metrics VALUES (1),(2),(3);")
        .unwrap();
    drop(connection);
    let catalog = root.join("catalog.json");
    std::fs::write(
        &catalog,
        r#"{"payments":{"name":"Payments DAG","type":"dag"}}"#,
    )
    .unwrap();
    let token = root.join("openmetadata-token");
    std::fs::write(&token, "plugin-test-token").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let mock_openmetadata = std::thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0);
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer plugin-test-token")
            );
            let line = request.lines().next().unwrap();
            let body = if index == 0 {
                assert!(line.starts_with("GET /api/v1/search/query?"), "{line}");
                assert!(line.contains("q=orders") && line.contains("index=table_search_index"));
                r#"{"hits":{"hits":[{"_source":{"name":"orders"}}]}}"#
            } else {
                assert!(
                    line.starts_with("GET /api/v1/tables/name/service.db.schema.orders?"),
                    "{line}"
                );
                assert!(line.contains("fields=columns%2Ctags%2Cowners"));
                r#"{"name":"orders","columns":[{"name":"id"}]}"#
            };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let package_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
    let db_manifest = package_root.join("sqlite/plugin.json");
    let meta_manifest = package_root.join("catalog-json/plugin.json");
    let quote = |path: &std::path::Path| {
        toml::Value::String(path.to_string_lossy().into_owned()).to_string()
    };
    let config = root.join("config.toml");
    std::fs::write(&config, format!(
        "[[plugin_databases]]\nid='sqlite_test'\nmanifest={}\nallowed_schemas=['main']\nmax_rows=2\nworkers=2\n[plugin_databases.settings]\npath={}\n[[plugin_metadata]]\nid='catalog_test'\nmanifest={}\n[plugin_metadata.settings]\npath={}\n[[plugin_metadata]]\nid='openmetadata_test'\nmanifest={}\n[plugin_metadata.settings]\nurl='http://127.0.0.1:{port}'\nallow_insecure=true\n[plugin_metadata.secrets]\ntoken={{kind='file',path={}}}\n",
        quote(&db_manifest), quote(&database), quote(&meta_manifest), quote(&catalog), quote(&package_root.join("openmetadata/plugin.json")), quote(&token)
    )).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_txttsql-mcp"))
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let initialize = send(
        &mut input,
        &mut output,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"plugin-test","version":"1"}}}),
    );
    assert!(initialize.get("result").is_some(), "{initialize}");
    writeln!(
        input,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    )
    .unwrap();
    input.flush().unwrap();
    let call = |input: &mut ChildStdin,
                output: &mut BufReader<ChildStdout>,
                id,
                name,
                arguments| {
        let response = send(
            input,
            output,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}),
        );
        serde_json::from_str::<Value>(response["result"]["content"][0]["text"].as_str().unwrap())
            .unwrap()
    };
    let sources = call(&mut input, &mut output, 2, "list_sources", json!({}));
    assert_eq!(sources["sources"][0]["plugin"], "sqlite");
    let sql = "SELECT id FROM main.metrics ORDER BY id";
    let checked = call(
        &mut input,
        &mut output,
        3,
        "validate_sql",
        json!({"source":"sqlite_test","sql":sql}),
    );
    assert_eq!(checked["ok"], true, "{checked}");
    assert!(checked["sql"].as_str().unwrap().ends_with("LIMIT 3"));
    let denied = call(
        &mut input,
        &mut output,
        4,
        "execute_sql",
        json!({"source":"sqlite_test","sql":"DELETE FROM main.metrics"}),
    );
    assert_eq!(denied["ok"], false, "{denied}");
    let result = call(
        &mut input,
        &mut output,
        5,
        "execute_sql",
        json!({"source":"sqlite_test","sql":sql}),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["result"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(result["result"]["truncated"], true);
    let tables = call(
        &mut input,
        &mut output,
        6,
        "list_tables",
        json!({"source":"sqlite_test"}),
    );
    assert_eq!(tables["discovered"][0]["table_name"], "metrics");
    let columns = call(
        &mut input,
        &mut output,
        7,
        "describe_table",
        json!({"source":"sqlite_test","table":"main.metrics"}),
    );
    assert_eq!(columns["columns"][0]["column_name"], "id");
    let found = call(
        &mut input,
        &mut output,
        8,
        "search_metadata",
        json!({"provider":"catalog_test","text":"pay"}),
    );
    assert_eq!(found["result"]["items"][0]["id"], "payments");
    let detail = call(
        &mut input,
        &mut output,
        9,
        "metadata_detail",
        json!({"provider":"catalog_test","id":"payments"}),
    );
    assert_eq!(detail["result"]["type"], "dag");
    let om_search = call(
        &mut input,
        &mut output,
        10,
        "search_metadata",
        json!({"provider":"openmetadata_test","text":"orders"}),
    );
    assert_eq!(
        om_search["result"]["hits"]["hits"][0]["_source"]["name"],
        "orders"
    );
    let om_detail = call(
        &mut input,
        &mut output,
        11,
        "metadata_detail",
        json!({"provider":"openmetadata_test","id":"service.db.schema.orders"}),
    );
    assert_eq!(om_detail["result"]["name"], "orders");
    mock_openmetadata.join().unwrap();
    drop(input);
    assert!(child.wait().unwrap().success());
    for path in [config, database, catalog, token] {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}
