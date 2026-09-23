"""Check the HTTPS broker and optionally drive it through a live PostgreSQL MCP call."""

import json
import os
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BROKER = ROOT / "examples/http_secret_broker.py"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/secret")
            self.end_headers()
            return
        if (
            self.path not in ("/secret", "/large")
            or self.headers.get("Authorization") != "Bearer test-auth"
        ):
            self.send_error(401)
            return
        body = b"x" * 8193 if self.path == "/large" else b"txttsql_test_only"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format_, *args):
        pass


class HttpBrokerTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.origin = f"http://127.0.0.1:{cls.server.server_port}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()

    def broker(self, path, allow_local=True, token="test-auth"):
        args = [sys.executable, str(BROKER), self.origin + path]
        if allow_local:
            args.append("--allow-insecure-local")
        return subprocess.run(
            args,
            capture_output=True,
            check=False,
            timeout=10,
            env={**os.environ, "TXTTSQL_BROKER_TOKEN": token},
        )

    def test_secret_and_fail_closed_cases(self):
        success = self.broker("/secret")
        self.assertEqual(success.returncode, 0, success.stderr)
        self.assertEqual(success.stdout, b"txttsql_test_only")
        for result in (
            self.broker("/secret", allow_local=False),
            self.broker("/redirect"),
            self.broker("/secret", token="wrong"),
            self.broker("/secret", token=""),
            self.broker("/large"),
        ):
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, b"")

    @unittest.skipUnless(shutil.which("openssl"), "OpenSSL is required for TLS test")
    def test_https_requires_trusted_ca(self):
        with tempfile.TemporaryDirectory(prefix="txttsql-broker-tls-") as directory:
            cert = Path(directory) / "server.crt"
            key = Path(directory) / "server.key"
            subprocess.run(
                [
                    "openssl",
                    "req",
                    "-x509",
                    "-newkey",
                    "rsa:2048",
                    "-nodes",
                    "-days",
                    "1",
                    "-keyout",
                    str(key),
                    "-out",
                    str(cert),
                    "-subj",
                    "/CN=127.0.0.1",
                    "-addext",
                    "subjectAltName=IP:127.0.0.1",
                ],
                capture_output=True,
                check=True,
                timeout=20,
            )
            server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(cert, key)
            server.socket = context.wrap_socket(server.socket, server_side=True)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                url = f"https://127.0.0.1:{server.server_port}/secret"
                args = [sys.executable, str(BROKER), url]
                env = {**os.environ, "TXTTSQL_BROKER_TOKEN": "test-auth"}
                denied = subprocess.run(
                    args, capture_output=True, check=False, timeout=10, env=env
                )
                self.assertNotEqual(denied.returncode, 0)
                self.assertEqual(denied.stdout, b"")
                accepted = subprocess.run(
                    args,
                    capture_output=True,
                    check=True,
                    timeout=10,
                    env={**env, "TXTTSQL_BROKER_CA_FILE": str(cert)},
                )
                self.assertEqual(accepted.stdout, b"txttsql_test_only")
            finally:
                server.shutdown()
                server.server_close()
                thread.join()

    @unittest.skipUnless(
        os.environ.get("TXTTSQL_TEST_PG_PORT"), "disposable PostgreSQL only"
    )
    def test_live_mcp_uses_http_broker(self):
        with tempfile.TemporaryDirectory(prefix="txttsql-http-broker-") as directory:
            config = Path(directory) / "test.toml"
            config.write_text(
                "[[sources]]\n"
                "id='http_broker_pg'\nkind='postgres'\nhost='127.0.0.1'\n"
                f"port={os.environ['TXTTSQL_TEST_PG_PORT']}\n"
                "database='analytics'\nuser='txttsql_reader'\n"
                "allowed_tables=['public.metrics']\nallow_insecure=true\n"
                "password={kind='broker',"
                f"program={json.dumps(sys.executable)},"
                f"args=[{json.dumps(str(BROKER))},{json.dumps(self.origin + '/secret')},'--allow-insecure-local']"
                "}\n",
                encoding="utf-8",
            )
            binary = ROOT / "target/debug/txttsql-mcp.exe"
            messages = [
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "http-broker-test", "version": "1"},
                    },
                },
                {"jsonrpc": "2.0", "method": "notifications/initialized"},
                {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "execute_sql",
                        "arguments": {
                            "source": "http_broker_pg",
                            "sql": "SELECT COUNT(*) AS n FROM public.metrics",
                        },
                    },
                },
            ]
            result = subprocess.run(
                [str(binary), "--config", str(config)],
                input="\n".join(map(json.dumps, messages)) + "\n",
                text=True,
                capture_output=True,
                check=True,
                timeout=20,
                env={**os.environ, "TXTTSQL_BROKER_TOKEN": "test-auth"},
            )
            responses = {
                message["id"]: message
                for line in result.stdout.splitlines()
                if (message := json.loads(line)).get("id") is not None
            }
            payload = json.loads(responses[2]["result"]["content"][0]["text"])
            self.assertTrue(payload["ok"], payload)
            self.assertEqual(payload["result"]["rows"][0]["n"], 3)


if __name__ == "__main__":
    unittest.main()
