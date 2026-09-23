"""Print one UTF-8 Generic Credential from Windows Credential Manager."""

import ctypes
import sys
from ctypes import wintypes


class Credential(ctypes.Structure):
    _fields_ = [
        ("Flags", wintypes.DWORD),
        ("Type", wintypes.DWORD),
        ("TargetName", wintypes.LPWSTR),
        ("Comment", wintypes.LPWSTR),
        ("LastWritten", wintypes.FILETIME),
        ("CredentialBlobSize", wintypes.DWORD),
        ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
        ("Persist", wintypes.DWORD),
        ("AttributeCount", wintypes.DWORD),
        ("Attributes", ctypes.c_void_p),
        ("TargetAlias", wintypes.LPWSTR),
        ("UserName", wintypes.LPWSTR),
    ]


def decode_blob(blob: bytes) -> str:
    # Generic Credential blobs are application-defined; cmdkey stores text as UTF-16LE.
    return blob.decode("utf-16-le" if b"\x00" in blob else "utf-8")


def main() -> int:
    if sys.platform != "win32" or len(sys.argv) != 2:
        print(
            "usage: windows_credential_broker.py TARGET (Windows only)", file=sys.stderr
        )
        return 2
    advapi = ctypes.WinDLL("Advapi32.dll", use_last_error=True)
    advapi.CredReadW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.POINTER(ctypes.POINTER(Credential)),
    ]
    advapi.CredReadW.restype = wintypes.BOOL
    advapi.CredFree.argtypes = [ctypes.c_void_p]
    pointer = ctypes.POINTER(Credential)()
    if not advapi.CredReadW(sys.argv[1], 1, 0, ctypes.byref(pointer)):
        print(
            f"credential unavailable (Windows error {ctypes.get_last_error()})",
            file=sys.stderr,
        )
        return 1
    try:
        size = pointer.contents.CredentialBlobSize
        if not 0 < size <= 8192:
            print("credential has invalid size", file=sys.stderr)
            return 1
        blob = ctypes.string_at(pointer.contents.CredentialBlob, size)
        value = decode_blob(blob)
        sys.stdout.buffer.write(value.encode("utf-8"))
        return 0
    finally:
        advapi.CredFree(pointer)


if __name__ == "__main__":
    raise SystemExit(main())
