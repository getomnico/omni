"""Unit tests for the crypto.encryption module.

Tests decryption of data encrypted by the TypeScript and Rust implementations,
and roundtrip compatibility.
"""

import base64
import json
import os

import pytest

# Set env vars before importing the module
os.environ["ENCRYPTION_KEY"] = "test_master_key_that_is_long_enough_32_chars"
os.environ["ENCRYPTION_SALT"] = "test_salt_16_chars"

from crypto.encryption import (
    _get_master_key,
    _derive_operation_key,
    decrypt,
    decrypt_config,
)

# Also test encrypt for roundtrip (import AESGCM for our own encrypt helper)
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF
from cryptography.hazmat.primitives.hashes import SHA256


def _encrypt_for_test(plaintext: str) -> dict[str, str]:
    """Encrypt using the same algorithm, for roundtrip testing."""
    master_key = _get_master_key()

    nonce = os.urandom(12)
    salt = os.urandom(16)

    operation_key = _derive_operation_key(master_key, salt)

    aesgcm = AESGCM(operation_key)
    ciphertext = aesgcm.encrypt(nonce, plaintext.encode("utf-8"), None)

    return {
        "data": base64.b64encode(ciphertext).decode(),
        "nonce": base64.b64encode(nonce).decode(),
        "salt": base64.b64encode(salt).decode(),
    }


class TestDecrypt:
    def test_roundtrip(self):
        plaintext = "Hello, World! This is sensitive data."
        encrypted = _encrypt_for_test(plaintext)
        assert decrypt(encrypted) == plaintext

    def test_roundtrip_json(self):
        config = {"apiKey": "sk-test-123", "model": "gpt-4"}
        encrypted = _encrypt_for_test(json.dumps(config))
        result = json.loads(decrypt(encrypted))
        assert result == config

    def test_different_encryptions_produce_different_ciphertext(self):
        plaintext = "Same data"
        e1 = _encrypt_for_test(plaintext)
        e2 = _encrypt_for_test(plaintext)

        assert e1["data"] != e2["data"]
        assert decrypt(e1) == plaintext
        assert decrypt(e2) == plaintext

    def test_invalid_nonce_length(self):
        encrypted = _encrypt_for_test("test")
        encrypted["nonce"] = base64.b64encode(b"\x00" * 5).decode()
        with pytest.raises(ValueError, match="Invalid nonce length"):
            decrypt(encrypted)

    def test_invalid_salt_length(self):
        encrypted = _encrypt_for_test("test")
        encrypted["salt"] = base64.b64encode(b"\x00" * 5).decode()
        with pytest.raises(ValueError, match="Invalid salt length"):
            decrypt(encrypted)

    def test_tampered_data_fails(self):
        encrypted = _encrypt_for_test("test data")
        # Tamper with ciphertext
        data = bytearray(base64.b64decode(encrypted["data"]))
        data[-1] ^= 0xFF
        encrypted["data"] = base64.b64encode(bytes(data)).decode()
        with pytest.raises(Exception):
            decrypt(encrypted)


class TestDecryptConfig:
    def test_encrypted_config(self):
        config = {"apiKey": "sk-test-123", "model": "gpt-4", "apiUrl": None}
        encrypted_data = _encrypt_for_test(json.dumps(config))
        db_value = {"encrypted_data": encrypted_data, "version": 1}

        result = decrypt_config(db_value)
        assert result == config

    def test_unencrypted_config_passthrough(self):
        config = {"apiKey": "sk-test-123", "model": "gpt-4"}
        result = decrypt_config(config)
        assert result == config

    def test_none_returns_empty_dict(self):
        assert decrypt_config(None) == {}

    def test_string_json_passthrough(self):
        config = {"apiKey": "sk-test-123"}
        result = decrypt_config(json.dumps(config))
        assert result == config

    def test_string_json_encrypted(self):
        config = {"apiKey": "sk-test-123"}
        encrypted_data = _encrypt_for_test(json.dumps(config))
        db_value = json.dumps({"encrypted_data": encrypted_data, "version": 1})

        result = decrypt_config(db_value)
        assert result == config
