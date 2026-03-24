"""
Encryption utilities compatible with the Rust EncryptionService (shared/src/encryption.rs).

Uses the same AES-256-GCM + HKDF-SHA256 scheme so that data encrypted by
the TypeScript web service or Rust indexer can be decrypted here, and vice versa.
"""

import base64
import json
import os
from typing import Any

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.hashes import SHA256

_master_key: bytes | None = None


def _get_master_key() -> bytes:
    global _master_key
    if _master_key is not None:
        return _master_key

    encryption_key = os.environ.get("ENCRYPTION_KEY", "")
    encryption_salt = os.environ.get("ENCRYPTION_SALT", "")

    if not encryption_key:
        raise ValueError("ENCRYPTION_KEY environment variable not set")
    if not encryption_salt:
        raise ValueError("ENCRYPTION_SALT environment variable not set")
    if len(encryption_key) < 32:
        raise ValueError("ENCRYPTION_KEY must be at least 32 characters long")
    if len(encryption_salt) < 16:
        raise ValueError("ENCRYPTION_SALT must be at least 16 characters long")

    _master_key = HKDF(
        algorithm=SHA256(),
        length=32,
        salt=encryption_salt.encode("utf-8"),
        info=b"omni-encryption-key",
    ).derive(encryption_key.encode("utf-8"))

    return _master_key


def _derive_operation_key(master_key: bytes, operation_salt: bytes) -> bytes:
    return HKDF(
        algorithm=SHA256(),
        length=32,
        salt=operation_salt,
        info=b"omni-operation-key",
    ).derive(master_key)


def decrypt(encrypted_data: dict[str, str]) -> str:
    """Decrypt an EncryptedData dict (with base64 data, nonce, salt fields)."""
    master_key = _get_master_key()

    combined = base64.b64decode(encrypted_data["data"])
    nonce = base64.b64decode(encrypted_data["nonce"])
    salt = base64.b64decode(encrypted_data["salt"])

    if len(nonce) != 12:
        raise ValueError("Invalid nonce length")
    if len(salt) != 16:
        raise ValueError("Invalid salt length")

    operation_key = _derive_operation_key(master_key, salt)

    # AESGCM expects ciphertext with appended auth tag (same as ring's format)
    aesgcm = AESGCM(operation_key)
    plaintext = aesgcm.decrypt(nonce, combined, None)

    return plaintext.decode("utf-8")


def decrypt_config(db_value: Any) -> dict:
    """Decrypt a config JSONB value if encrypted, or return as-is for backward compat."""
    if db_value is None:
        return {}

    if isinstance(db_value, str):
        db_value = json.loads(db_value)

    if isinstance(db_value, dict) and "encrypted_data" in db_value:
        plaintext = decrypt(db_value["encrypted_data"])
        return json.loads(plaintext)

    return db_value
