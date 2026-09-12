"""Independent public toy vector: cryptography Argon2id + system libsodium AEAD.

No real wallet material is read. Requires Python cryptography >=44 and libsodium.
This intentionally deterministic generator is test tooling, never a backup API.
"""
import ctypes
import ctypes.util
import json
from pathlib import Path

import cryptography
from cryptography.hazmat.primitives.kdf.argon2 import Argon2id

sodium = ctypes.CDLL(ctypes.util.find_library("sodium"))
sodium.sodium_init.restype = ctypes.c_int
assert sodium.sodium_init() >= 0
sodium.sodium_version_string.restype = ctypes.c_char_p
encrypt = sodium.crypto_aead_xchacha20poly1305_ietf_encrypt
encrypt.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulonglong), ctypes.c_void_p,
                    ctypes.c_ulonglong, ctypes.c_void_p, ctypes.c_ulonglong,
                    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p]
encrypt.restype = ctypes.c_int

seed = bytes(range(17))
password = b"public-backup-vector-password"
salt = bytes(range(16))
nonce = bytes(range(24))
memory, iterations, lanes = 65536, 3, 4
coin, account = 969, 7
header = b"QUAISEED" + bytes([1, 1, 1, 0])
header += b"".join(value.to_bytes(4, "big") for value in [memory, iterations, lanes])
header += salt + nonce
assert len(header) == 64
payload = bytes([len(seed)]) + coin.to_bytes(2, "big") + account.to_bytes(4, "big")
payload += seed + bytes(64 - len(seed))
key = Argon2id(salt=salt, length=32, iterations=iterations, lanes=lanes,
               memory_cost=memory).derive(password)
ciphertext = ctypes.create_string_buffer(len(payload) + 16)
length = ctypes.c_ulonglong()
assert encrypt(ciphertext, ctypes.byref(length), payload, len(payload),
               header, len(header), None, nonce, key) == 0
assert length.value == len(payload) + 16
vector = {
    "format": "QUAISEED-v1", "publicTestSecretsOnly": True,
    "generator": {"cryptography": cryptography.__version__,
                  "libsodium": sodium.sodium_version_string().decode()},
    "seed": seed.hex(), "password": password.decode(), "salt": salt.hex(),
    "nonce": nonce.hex(), "coin": coin, "account": account,
    "memoryKiB": memory, "iterations": iterations, "lanes": lanes,
    "key": key.hex(), "envelope": (header + ciphertext.raw).hex(),
}
Path(__file__).with_name("backup-vector.json").write_text(json.dumps(vector, indent=2) + "\n")
print("Generated independent Argon2id/libsodium public seed-backup vector.")
