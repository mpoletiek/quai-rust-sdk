"""Independent public-only QUAIWALT v2 registered-channel vector. No real wallet data is read.
Requires cryptography >=44 and system libsodium; deterministic test tooling only.
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
pair = json.loads((Path(__file__).parents[2] / "quai-payments/tests/fixtures/quais-payments.json").read_text())["pairs"][0]
seed = bytes.fromhex(pair["senderSeed"][2:])
password = b"public-full-wallet-vector-password"
salt, nonce = bytes(range(16)), bytes(range(24))
memory, iterations, lanes = 65536, 3, 4
payload = (1).to_bytes(4, "big")         # registered-channel extension
payload += (1).to_bytes(2, "big")       # one secret origin
payload += bytes([1]) + len(seed).to_bytes(2, "big") + seed
payload += (1).to_bytes(2, "big")       # one scope
payload += (15000).to_bytes(32, "big") + bytes([1]) * 32 + bytes([0])
payload += bytes(16)                     # four empty public-state collections
metadata = json.dumps({"schemaVersion": 1, "scheme": "bip47-quai-969-v1",
                       "account": 0, "localPaymentCode": pair["senderCode"],
                       "counterpartyPaymentCode": pair["receiverCode"],
                       "sendNext": [10000] + [0] * 8, "receiveNext": [0] * 9},
                      separators=(",", ":")).encode()
payload += (1).to_bytes(4, "big")       # one registered channel
payload += (15000).to_bytes(32, "big") + bytes([1]) * 32
payload += bytes.fromhex(pair["senderPayload"][2:])
payload += bytes.fromhex(pair["receiverPayload"][2:])
payload += bytes(4) + (1).to_bytes(8, "big")  # account and local generation
payload += len(metadata).to_bytes(2, "big") + metadata
payload += bytes(4)                       # no exposed addresses
header = b"QUAIWALT" + bytes([2, 1, 1, 0])
header += b"".join(value.to_bytes(4, "big") for value in [memory, iterations, lanes])
header += salt + nonce + len(payload).to_bytes(4, "big")
assert len(header) == 68 and len(payload) < 4096
key = Argon2id(salt=salt, length=32, iterations=iterations, lanes=lanes,
               memory_cost=memory).derive(password)
ciphertext = ctypes.create_string_buffer(len(payload) + 16)
length = ctypes.c_ulonglong()
assert encrypt(ciphertext, ctypes.byref(length), payload, len(payload), header,
               len(header), None, nonce, key) == 0
assert length.value == len(payload) + 16
vector = {
    "format": "QUAIWALT-v2", "publicTestSecretsOnly": True,
    "generator": {"cryptography": cryptography.__version__,
                  "libsodium": sodium.sodium_version_string().decode()},
    "seed": seed.hex(), "password": password.decode(),
    "salt": salt.hex(), "nonce": nonce.hex(), "memoryKiB": memory,
    "iterations": iterations, "lanes": lanes, "key": key.hex(),
    "reference": "quais@1.0.0-alpha.57 payment-code fixture",
    "peerCode": pair["receiverCode"], "metadata": metadata.decode(),
    "plaintext": payload.hex(), "envelope": (header + ciphertext.raw).hex(),
}
Path(__file__).with_name("full-backup-v2-vector.json").write_text(json.dumps(vector, indent=2) + "\n")
