"""Check that the non-root Docker image can use a fresh memory volume."""

import json
import subprocess
import tempfile
import time
import uuid
from pathlib import Path

name = f"txttsql-memory-smoke-{uuid.uuid4().hex[:12]}"
subprocess.run(["docker", "volume", "create", name], check=True, capture_output=True)
try:
    permissions = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--entrypoint",
            "stat",
            "--mount",
            f"type=volume,source={name},target=/data",
            "txttsql-mcp:ci",
            "-c",
            "%a %u:%g",
            "/data",
        ],
        text=True,
        capture_output=True,
        timeout=30,
        check=True,
    ).stdout.strip()
    assert permissions == "700 65532:65532", permissions
    with tempfile.TemporaryDirectory() as directory:
        config = Path(directory, "config.toml")
        config.write_text(
            "[[sources]]\n"
            "id='db'\nkind='postgres'\nhost='localhost'\nport=5432\n"
            "database='db'\nuser='reader'\n"
            "password={kind='env',name='UNUSED_PASSWORD'}\n"
            "allowed_schemas=['public']\n"
            "[memory]\nenabled=true\npath='/data/queries.sqlite'\n"
        )
        messages = [
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "docker-smoke", "version": "1"},
                },
            },
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "search_saved_queries",
                    "arguments": {"text": "x"},
                },
            },
        ]
        result = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "-i",
                "--name",
                name,
                "--mount",
                f"type=bind,source={config.resolve()},target=/app/config.toml,readonly",
                "--mount",
                f"type=volume,source={name},target=/data",
                "txttsql-mcp:ci",
            ],
            input="\n".join(map(json.dumps, messages)) + "\n",
            text=True,
            capture_output=True,
            timeout=30,
            check=True,
        )
        responses = [json.loads(line) for line in result.stdout.splitlines()]
        call = next(item for item in responses if item.get("id") == 2)
        payload = json.loads(call["result"]["content"][0]["text"])
        assert payload == {"ok": True, "queries": []}, payload
        print("Docker memory smoke passed")
finally:
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    for attempt in range(10):
        removed = subprocess.run(
            ["docker", "volume", "rm", name], capture_output=True, check=False
        )
        if removed.returncode == 0:
            break
        time.sleep(1)
    else:
        raise RuntimeError(f"disposable volume could not be removed: {name}")
