"""Fetch a text secret for SecretRef::Broker from an authenticated HTTPS endpoint."""

import os
import ssl
import sys
from urllib import request
from urllib.parse import urlsplit


class NoRedirect(request.HTTPRedirectHandler):
    def redirect_request(self, request_, fp, code, msg, headers, newurl):
        return None


def main() -> int:
    local_http = len(sys.argv) == 3 and sys.argv[2] == "--allow-insecure-local"
    if len(sys.argv) not in (2, 3) or (len(sys.argv) == 3 and not local_http):
        print(
            "usage: http_secret_broker.py URL [--allow-insecure-local]", file=sys.stderr
        )
        return 2
    url = urlsplit(sys.argv[1])
    if (
        not url.hostname
        or url.username
        or url.password
        or url.fragment
        or (
            url.scheme != "https"
            and not (
                local_http
                and url.scheme == "http"
                and url.hostname in {"127.0.0.1", "::1"}
            )
        )
    ):
        print("credential endpoint must use HTTPS", file=sys.stderr)
        return 2
    token = os.environ.get("TXTTSQL_BROKER_TOKEN")
    if not token:
        print("TXTTSQL_BROKER_TOKEN is missing", file=sys.stderr)
        return 2
    try:
        context = ssl.create_default_context(
            cafile=os.environ.get("TXTTSQL_BROKER_CA_FILE")
        )
        opener = request.build_opener(
            request.ProxyHandler({}),
            NoRedirect(),
            request.HTTPSHandler(context=context),
        )
        http_request = request.Request(
            sys.argv[1], headers={"Authorization": f"Bearer {token}"}
        )
        with opener.open(http_request, timeout=5) as response:
            secret = response.read(8193)
        if not 0 < len(secret) <= 8192:
            raise ValueError("invalid credential size")
        secret.decode("utf-8")
    except (OSError, UnicodeError, ValueError):
        print("credential request failed", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(secret)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
