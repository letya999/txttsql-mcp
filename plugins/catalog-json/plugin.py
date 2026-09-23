"""Independent JSON catalog metadata plugin for txttsql-plugin/1."""

import json
import sys
from pathlib import Path


def handle(request):
    method = request["method"]
    payload = request["payload"]
    if method == "hello":
        return {
            "plugin": "catalog-json",
            "kind": "metadata",
            "capabilities": ["search", "detail"],
        }
    catalog = json.loads(Path(payload["settings"]["path"]).read_text(encoding="utf-8"))
    params = payload["params"]
    if method == "search":
        needle = params["text"].casefold()
        return {
            "items": [
                {"id": key, "name": value.get("name", key)}
                for key, value in catalog.items()
                if needle in key.casefold()
                or needle in value.get("name", "").casefold()
            ][:20]
        }
    if method == "detail":
        return catalog[params["id"]]
    raise ValueError("unknown method")


for line in sys.stdin:
    request = {}
    try:
        request = json.loads(line)
        if request["protocol"] != 1:
            raise ValueError("protocol mismatch")
        response = {
            "protocol": 1,
            "id": request["id"],
            "ok": True,
            "result": handle(request),
        }
    except (KeyError, ValueError, TypeError, OSError) as error:
        response = {
            "protocol": 1,
            "id": request.get("id", 0),
            "ok": False,
            "error": type(error).__name__,
        }
    print(json.dumps(response, ensure_ascii=False), flush=True)
