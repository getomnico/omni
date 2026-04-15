"""Content storage abstraction.

Python translation of the Rust `ObjectStorage` trait in `shared/src/storage/`. One base
class, two concrete implementations (Postgres and S3). Each implementation owns all of
its DB access — no intermediate repository.
"""

import asyncio
import hashlib
import logging
import os
from abc import ABC, abstractmethod
from dataclasses import dataclass
from typing import Optional

import boto3
import ulid

from db.connection import get_db_pool

logger = logging.getLogger(__name__)


@dataclass
class ContentMetadata:
    content_type: Optional[str]
    size_bytes: int
    sha256_hash: str


class ContentStorage(ABC):
    """Content storage contract. Mirrors shared::storage::ObjectStorage on the Rust side."""

    @abstractmethod
    async def put(self, content: bytes, content_type: Optional[str]) -> str:
        """Store bytes, return a new content_id."""

    @abstractmethod
    async def get_bytes(self, content_id: str) -> bytes:
        """Fetch raw bytes for a content_id."""

    @abstractmethod
    async def delete(self, content_id: str) -> None:
        """Delete a content blob. Raises if not found."""

    @abstractmethod
    async def get_metadata(self, content_id: str) -> ContentMetadata:
        """Fetch metadata without reading the bytes."""

    async def put_text(
        self, text: str, content_type: Optional[str] = "text/plain"
    ) -> str:
        return await self.put(text.encode("utf-8"), content_type)

    async def get_text(self, content_id: str) -> str:
        return (await self.get_bytes(content_id)).decode("utf-8")


class PostgresContentStorage(ContentStorage):
    """Stores bytes directly in the `content_blobs.content` BYTEA column."""

    def __init__(self) -> None:
        logger.info("Initialized Postgres content storage")

    async def put(self, content: bytes, content_type: Optional[str]) -> str:
        content_id = str(ulid.ULID())
        sha256 = hashlib.sha256(content).hexdigest()
        pool = await get_db_pool()
        await pool.execute(
            """
            INSERT INTO content_blobs
                (id, content, content_type, size_bytes, sha256_hash, storage_backend)
            VALUES ($1, $2, $3, $4, $5, 'postgres')
            """,
            content_id,
            content,
            content_type,
            len(content),
            sha256,
        )
        return content_id

    async def get_bytes(self, content_id: str) -> bytes:
        pool = await get_db_pool()
        row = await pool.fetchrow(
            """
            SELECT content, storage_backend
            FROM content_blobs
            WHERE id = $1
            """,
            content_id,
        )
        if not row:
            raise ValueError(f"Content not found: {content_id}")
        if row["storage_backend"] != "postgres":
            raise ValueError(
                f"Content {content_id} has storage_backend "
                f"'{row['storage_backend']}', expected 'postgres'"
            )
        if row["content"] is None:
            raise ValueError(f"Content is null for id: {content_id}")
        return row["content"]

    async def delete(self, content_id: str) -> None:
        pool = await get_db_pool()
        result = await pool.execute(
            "DELETE FROM content_blobs WHERE id = $1", content_id
        )
        if result.endswith(" 0"):
            raise ValueError(f"Content not found: {content_id}")

    async def get_metadata(self, content_id: str) -> ContentMetadata:
        pool = await get_db_pool()
        row = await pool.fetchrow(
            """
            SELECT content_type, size_bytes, sha256_hash
            FROM content_blobs
            WHERE id = $1
            """,
            content_id,
        )
        if not row:
            raise ValueError(f"Content not found: {content_id}")
        return ContentMetadata(
            content_type=row["content_type"],
            size_bytes=row["size_bytes"],
            sha256_hash=row["sha256_hash"],
        )


class S3ContentStorage(ContentStorage):
    """Stores bytes in S3; tracks metadata in `content_blobs`."""

    def __init__(self, bucket: str, region: Optional[str] = None) -> None:
        self.bucket = bucket
        self.s3_client = (
            boto3.client("s3", region_name=region) if region else boto3.client("s3")
        )
        logger.info(f"Initialized S3 content storage for bucket: {bucket}")

    async def put(self, content: bytes, content_type: Optional[str]) -> str:
        content_id = str(ulid.ULID())
        storage_key = content_id
        sha256 = hashlib.sha256(content).hexdigest()

        loop = asyncio.get_event_loop()
        await loop.run_in_executor(
            None,
            lambda: self.s3_client.put_object(
                Bucket=self.bucket,
                Key=storage_key,
                Body=content,
                ContentType=content_type or "application/octet-stream",
            ),
        )

        pool = await get_db_pool()
        await pool.execute(
            """
            INSERT INTO content_blobs
                (id, storage_key, content_type, size_bytes, sha256_hash, storage_backend)
            VALUES ($1, $2, $3, $4, $5, 's3')
            """,
            content_id,
            storage_key,
            content_type,
            len(content),
            sha256,
        )
        return content_id

    async def _get_storage_key(self, content_id: str) -> str:
        pool = await get_db_pool()
        row = await pool.fetchrow(
            """
            SELECT storage_key, storage_backend
            FROM content_blobs
            WHERE id = $1
            """,
            content_id,
        )
        if not row:
            raise ValueError(f"Content not found: {content_id}")
        if row["storage_backend"] != "s3":
            raise ValueError(
                f"Content {content_id} has storage_backend "
                f"'{row['storage_backend']}', expected 's3'"
            )
        if not row["storage_key"]:
            raise ValueError(f"Storage key is null for id: {content_id}")
        return row["storage_key"]

    async def get_bytes(self, content_id: str) -> bytes:
        storage_key = await self._get_storage_key(content_id)
        loop = asyncio.get_event_loop()
        response = await loop.run_in_executor(
            None,
            lambda: self.s3_client.get_object(Bucket=self.bucket, Key=storage_key),
        )
        return response["Body"].read()

    async def delete(self, content_id: str) -> None:
        storage_key = await self._get_storage_key(content_id)
        loop = asyncio.get_event_loop()
        await loop.run_in_executor(
            None,
            lambda: self.s3_client.delete_object(Bucket=self.bucket, Key=storage_key),
        )
        pool = await get_db_pool()
        await pool.execute("DELETE FROM content_blobs WHERE id = $1", content_id)

    async def get_metadata(self, content_id: str) -> ContentMetadata:
        pool = await get_db_pool()
        row = await pool.fetchrow(
            """
            SELECT content_type, size_bytes, sha256_hash
            FROM content_blobs
            WHERE id = $1
            """,
            content_id,
        )
        if not row:
            raise ValueError(f"Content not found: {content_id}")
        return ContentMetadata(
            content_type=row["content_type"],
            size_bytes=row["size_bytes"],
            sha256_hash=row["sha256_hash"],
        )


def create_content_storage() -> ContentStorage:
    """Factory: pick backend from STORAGE_BACKEND env var."""
    storage_backend = os.getenv("STORAGE_BACKEND", "postgres")

    if storage_backend == "s3":
        bucket = os.getenv("S3_BUCKET")
        if not bucket:
            raise ValueError(
                "S3_BUCKET environment variable is required when STORAGE_BACKEND=s3"
            )
        region = os.getenv("S3_REGION") or os.getenv("AWS_REGION")
        return S3ContentStorage(bucket, region)

    if storage_backend == "postgres":
        return PostgresContentStorage()

    raise ValueError(f"Unsupported storage backend for AI service: {storage_backend}")
