"""
Content storage client for accessing document content from S3 or PostgreSQL
"""

import asyncio
import logging
import os
from typing import Optional, Protocol

import boto3
import ulid

from db.content_blobs import ContentBlobsRepository

logger = logging.getLogger(__name__)


class ContentStorageBackend(Protocol):
    """Protocol implemented by both S3 and Postgres storage backends."""

    async def get_text(self, content_id: str) -> str: ...
    async def get_bytes(self, content_id: str) -> bytes: ...
    async def put(self, content: bytes, content_type: str) -> str: ...


class ContentStorage:
    """Client for fetching and storing document content in S3"""

    def __init__(
        self,
        bucket: str,
        content_blobs_repo: ContentBlobsRepository,
        region: Optional[str] = None,
    ):
        self.bucket = bucket
        self.content_blobs_repo = content_blobs_repo
        if region:
            self.s3_client = boto3.client("s3", region_name=region)
        else:
            self.s3_client = boto3.client("s3")
        logger.info(f"Initialized content storage client for bucket: {bucket}")

    async def get_text(self, content_id: str) -> str:
        return (await self.get_bytes(content_id)).decode("utf-8")

    async def get_bytes(self, content_id: str) -> bytes:
        blob = await self.content_blobs_repo.get_by_id(content_id)
        if not blob:
            raise ValueError(f"Content not found for id: {content_id}")
        if not blob.storage_key:
            raise ValueError(f"Storage key is null for content id: {content_id}")

        loop = asyncio.get_event_loop()
        response = await loop.run_in_executor(
            None,
            lambda: self.s3_client.get_object(Bucket=self.bucket, Key=blob.storage_key),
        )
        return response["Body"].read()

    async def put(self, content: bytes, content_type: str) -> str:
        content_id = str(ulid.ULID())
        storage_key = content_id

        loop = asyncio.get_event_loop()
        await loop.run_in_executor(
            None,
            lambda: self.s3_client.put_object(
                Bucket=self.bucket,
                Key=storage_key,
                Body=content,
                ContentType=content_type,
            ),
        )
        await self.content_blobs_repo.insert_s3(
            content_id, storage_key, content_type, len(content)
        )
        return content_id


class PostgresContentStorage:
    """Client for fetching and storing document content in PostgreSQL"""

    def __init__(self, content_blobs_repo: ContentBlobsRepository):
        self.content_blobs_repo = content_blobs_repo
        logger.info("Initialized PostgreSQL content storage client")

    async def get_text(self, content_id: str) -> str:
        return (await self.get_bytes(content_id)).decode("utf-8")

    async def get_bytes(self, content_id: str) -> bytes:
        blob = await self.content_blobs_repo.get_by_id(content_id)

        if not blob:
            raise ValueError(f"Content not found for id: {content_id}")

        if blob.storage_backend != "postgres":
            raise ValueError(
                f"Content {content_id} has storage_backend '{blob.storage_backend}', expected 'postgres'"
            )

        if blob.content is None:
            raise ValueError(f"Content is null for id: {content_id}")

        return blob.content

    async def put(self, content: bytes, content_type: str) -> str:
        content_id = str(ulid.ULID())
        await self.content_blobs_repo.insert_postgres(content_id, content, content_type)
        return content_id


def create_content_storage() -> ContentStorageBackend:
    """Factory function to create content storage from environment variables"""
    storage_backend = os.getenv("STORAGE_BACKEND", "postgres")
    content_blobs_repo = ContentBlobsRepository()

    if storage_backend == "s3":
        bucket = os.getenv("S3_BUCKET")
        if not bucket:
            raise ValueError(
                "S3_BUCKET environment variable is required when STORAGE_BACKEND=s3"
            )

        region = os.getenv("S3_REGION") or os.getenv("AWS_REGION")
        return ContentStorage(bucket, content_blobs_repo, region)

    elif storage_backend == "postgres":
        return PostgresContentStorage(content_blobs_repo)

    else:
        raise ValueError(
            f"Unsupported storage backend for AI service: {storage_backend}"
        )
