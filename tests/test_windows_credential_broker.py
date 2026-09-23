"""Exercise the broker with one disposable Windows Generic Credential."""

import ctypes
import io
import subprocess
import sys
import unittest
import uuid
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "examples"))
import windows_credential_broker as broker
from windows_credential_broker import Credential, decode_blob


class CredentialBlobTest(unittest.TestCase):
    def test_generic_blob_encodings(self):
        self.assertEqual(decode_blob(b"test-value"), "test-value")
        self.assertEqual(decode_blob("test-value".encode("utf-16-le")), "test-value")

    def test_credread_success_contract(self):
        secret = b"test-value"
        blob = ctypes.create_string_buffer(secret)
        credential = Credential()
        credential.CredentialBlobSize = len(secret)
        credential.CredentialBlob = ctypes.cast(blob, ctypes.POINTER(ctypes.c_ubyte))
        pointer = ctypes.pointer(credential)
        freed = []

        def cred_read(target, kind, flags, output):
            self.assertEqual((target, kind, flags), ("test-target", 1, 0))
            ctypes.cast(output, ctypes.POINTER(ctypes.POINTER(Credential)))[0] = pointer
            return 1

        def cred_free(value):
            freed.append(value)

        sink = io.BytesIO()
        stdout = io.TextIOWrapper(sink, encoding="utf-8")
        api = SimpleNamespace(CredReadW=cred_read, CredFree=cred_free)
        with (
            patch.object(broker.sys, "platform", "win32"),
            patch.object(broker.sys, "argv", ["broker.py", "test-target"]),
            patch.object(broker.sys, "stdout", stdout),
            patch.object(broker.ctypes, "WinDLL", return_value=api, create=True),
        ):
            self.assertEqual(broker.main(), 0)
        stdout.flush()
        self.assertEqual(sink.getvalue(), secret)
        self.assertEqual(len(freed), 1)


@unittest.skipUnless(sys.platform == "win32", "Windows Credential Manager only")
class CredentialBrokerTest(unittest.TestCase):
    def test_missing_target_fails_without_output(self):
        broker = (
            Path(__file__).resolve().parents[1]
            / "examples/windows_credential_broker.py"
        )
        result = subprocess.run(
            [sys.executable, str(broker), f"txttsql-mcp/missing-{uuid.uuid4().hex}"],
            capture_output=True,
            check=False,
            timeout=5,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"")

    def test_generic_credential_round_trip(self):
        advapi = ctypes.WinDLL("Advapi32.dll", use_last_error=True)
        advapi.CredWriteW.argtypes = [ctypes.POINTER(Credential), ctypes.c_uint32]
        advapi.CredWriteW.restype = ctypes.c_int
        advapi.CredDeleteW.argtypes = [
            ctypes.c_wchar_p,
            ctypes.c_uint32,
            ctypes.c_uint32,
        ]
        advapi.CredDeleteW.restype = ctypes.c_int
        target = f"txttsql-mcp/test-{uuid.uuid4().hex}"
        password = b"txttsql_test_only"
        blob = ctypes.create_string_buffer(password)
        credential = Credential()
        credential.Type = 1  # CRED_TYPE_GENERIC
        credential.TargetName = target
        credential.CredentialBlobSize = len(password)
        credential.CredentialBlob = ctypes.cast(blob, ctypes.POINTER(ctypes.c_ubyte))
        credential.Persist = 1  # CRED_PERSIST_SESSION
        credential.UserName = "txttsql_reader"
        if not advapi.CredWriteW(ctypes.byref(credential), 0):
            error = ctypes.get_last_error()
            if error == 8:
                self.skipTest(
                    "Windows Credential Manager cannot create a test credential: error 8"
                )
            self.fail(f"CredWriteW failed: {error}")
        try:
            broker = (
                Path(__file__).resolve().parents[1]
                / "examples/windows_credential_broker.py"
            )
            result = subprocess.run(
                [sys.executable, str(broker), target],
                capture_output=True,
                check=True,
                timeout=5,
            )
            self.assertEqual(result.stdout, password)
        finally:
            self.assertTrue(advapi.CredDeleteW(target, 1, 0), ctypes.get_last_error())


if __name__ == "__main__":
    unittest.main()
