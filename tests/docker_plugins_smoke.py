"""Exercise installed database and metadata plugins in the Docker image."""

import json
import os
import sqlite3
import subprocess
import tempfile
import time
import uuid
from contextlib import closing
from pathlib import Path


def call(request_id, name, arguments):
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }


temporary = tempfile.TemporaryDirectory(prefix="txttsql-plugin-smoke-")
try:
    root = Path(temporary.name)
    os.chmod(root, 0o755)
    with closing(sqlite3.connect(root / "metrics.sqlite")) as db:
        db.executescript(
            "CREATE TABLE metrics(id INTEGER); INSERT INTO metrics VALUES (1),(2),(3);"
        )
        db.commit()
    (root / "catalog.json").write_text(
        json.dumps({"payments": {"name": "Payments DAG"}}), encoding="utf-8"
    )
    (root / "config.toml").write_text(
        "[[plugin_databases]]\n"
        "id='sqlite_test'\n"
        "manifest='/opt/txttsql/plugins/sqlite/plugin.json'\n"
        "allowed_schemas=['main']\nmax_rows=2\n"
        "[plugin_databases.settings]\npath='/fixtures/metrics.sqlite'\n"
        "[[plugin_metadata]]\nid='catalog_test'\n"
        "manifest='/opt/txttsql/plugins/catalog-json/plugin.json'\n"
        "[plugin_metadata.settings]\npath='/fixtures/catalog.json'\n",
        encoding="utf-8",
    )
    messages = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "docker-plugin-smoke", "version": "1"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        call(
            2,
            "execute_sql",
            {"source": "sqlite_test", "sql": "SELECT id FROM main.metrics ORDER BY id"},
        ),
        call(
            3,
            "execute_sql",
            {"source": "sqlite_test", "sql": "DELETE FROM main.metrics"},
        ),
        call(4, "search_metadata", {"provider": "catalog_test", "text": "pay"}),
    ]
    name = f"txttsql-plugin-smoke-{uuid.uuid4().hex[:12]}"
    try:
        result = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "-i",
                "--name",
                name,
                "--mount",
                f"type=bind,source={root.resolve()},target=/fixtures,readonly",
                "txttsql-mcp:plugins",
                "--config",
                "/fixtures/config.toml",
            ],
            input="\n".join(map(json.dumps, messages)) + "\n",
            text=True,
            capture_output=True,
            timeout=40,
            check=True,
        )
        responses = {
            message["id"]: message
            for line in result.stdout.splitlines()
            if (message := json.loads(line)).get("id") is not None
        }
        assert "result" in responses[1], responses[1]
        payload = lambda request_id: json.loads(
            responses[request_id]["result"]["content"][0]["text"]
        )
        rows = payload(2)
        assert (
            rows["ok"]
            and len(rows["result"]["rows"]) == 2
            and rows["result"]["truncated"]
        ), rows
        assert payload(3)["ok"] is False
        assert payload(4)["result"]["items"][0]["id"] == "payments"
        print("Docker plugin smoke passed")
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
finally:
    assert (
        Path(temporary.name).resolve().parent == Path(tempfile.gettempdir()).resolve()
    )
    for attempt in range(10):
        try:
            temporary.cleanup()
            break
        except PermissionError:
            if attempt == 9:
                raise
            time.sleep(1)
