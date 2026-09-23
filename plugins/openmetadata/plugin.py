"""OpenMetadata REST adapter for txttsql-plugin/1 (Python stdlib only)."""

import json
import ssl
import sys
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, HTTPSHandler, Request, build_opener

MAX_RESPONSE = 4 * 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        return None


def fetch(settings, secrets, path, query):
    base = settings["url"]
    parsed = urlsplit(base)
    if (
        parsed.scheme not in ("https", "http")
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
        or parsed.path not in ("", "/")
        or (parsed.scheme == "http" and not settings.get("allow_insecure", False))
    ):
        raise ValueError("invalid OpenMetadata origin")
    url = base.rstrip("/") + path + "?" + urlencode(query)
    context = ssl.create_default_context(cafile=settings.get("ca_file"))
    opener = build_opener(NoRedirect, HTTPSHandler(context=context))
    request = Request(url, headers={"Authorization": "Bearer " + secrets["token"]})
    with opener.open(request, timeout=15) as response:
        body = response.read(MAX_RESPONSE + 1)
    if len(body) > MAX_RESPONSE:
        raise ValueError("OpenMetadata response is too large")
    return json.loads(body)


def handle(request):
    method = request["method"]
    if method == "hello":
        return {
            "plugin": "openmetadata",
            "kind": "metadata",
            "capabilities": ["search", "detail"],
        }
    payload = request["payload"]
    params = payload["params"]
    if method == "search":
        text = params["text"]
        if not text.strip() or len(text) > 200:
            raise ValueError("invalid search text")
        return fetch(
            payload["settings"],
            payload["secrets"],
            "/api/v1/search/query",
            {"q": text, "index": "table_search_index", "size": "20"},
        )
    if method == "detail":
        name = params["id"]
        if not name or len(name) > 300:
            raise ValueError("invalid table name")
        return fetch(
            payload["settings"],
            payload["secrets"],
            "/api/v1/tables/name/" + quote(name, safe=""),
            {"fields": "columns,tags,owners"},
        )
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
