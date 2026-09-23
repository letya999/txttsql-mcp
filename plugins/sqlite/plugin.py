"""Independent SQLite database plugin for txttsql-plugin/1 (Python stdlib only)."""

import json
import sqlite3
import sys
import time
from contextlib import closing
from pathlib import Path


def connect(settings):
    uri = Path(settings["path"]).resolve().as_uri() + "?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    connection.execute("PRAGMA query_only=ON")
    return connection


def handle(request):
    method = request["method"]
    payload = request["payload"]
    if method == "hello":
        return {
            "plugin": "sqlite",
            "kind": "database",
            "capabilities": ["execute", "list_tables", "describe_table"],
        }
    settings = payload["settings"]
    params = payload["params"]
    with closing(connect(settings)) as connection:
        if method == "list_tables":
            rows = connection.execute(
                "SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name"
            ).fetchmany(1001)
            return {"tables": [f"main.{name}" for (name,) in rows]}
        if method == "describe_table":
            table = params["table"].split(".", 1)[1]
            rows = connection.execute(
                "SELECT name, type FROM pragma_table_info(?) ORDER BY cid", (table,)
            ).fetchmany(1001)
            return {
                "columns": [
                    {"column_name": name, "data_type": kind} for name, kind in rows
                ],
                "truncated": len(rows) > 1000,
            }
        if method == "execute":
            started = time.monotonic()
            cursor = connection.execute(params["sql"])
            names = [column[0] for column in cursor.description]
            limit = params["row_cap"]
            rows = cursor.fetchmany(limit + 1)
            return {
                "rows": [dict(zip(names, row, strict=True)) for row in rows[:limit]],
                "truncated": len(rows) > limit,
                "elapsed_ms": int((time.monotonic() - started) * 1000),
            }
    raise ValueError("unknown method")


for line in sys.stdin:
    request = {}
    try:
        request = json.loads(line)
        if request["protocol"] != 1:
            raise ValueError("protocol mismatch")
        result = handle(request)
        response = {"protocol": 1, "id": request["id"], "ok": True, "result": result}
    except (KeyError, ValueError, TypeError, sqlite3.Error, OSError) as error:
        response = {
            "protocol": 1,
            "id": request.get("id", 0),
            "ok": False,
            "error": type(error).__name__,
        }
    print(json.dumps(response, ensure_ascii=False), flush=True)
