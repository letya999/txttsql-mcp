use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
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
