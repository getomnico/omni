"""Load an EnterpriseRAG-Bench slice directly into Omni's connector_events_queue.

Bypasses the connector / connector-manager path: writes minimal users, sources,
sync_runs, and a local-TEI embedding provider record, then inserts content_blobs
and DocumentCreated events. The indexer drains the queue and the AI service
generates embeddings, exactly as in the production path.

Layout expected under --data-dir:
    confluence/dsid_<hex>...txt
    jira/dsid_<hex>...txt
First line of each file is the title; full file body is the content.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import re
import sys
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

import psycopg
from psycopg import sql
from psycopg.types.json import Jsonb
from tqdm import tqdm
from ulid import ULID

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
)
log = logging.getLogger("load_corpus")

DSID_RE = re.compile(r"^(dsid_[0-9a-f]+)")
BENCH_USER_EMAIL = "bench@omni.local"
BENCH_USER_PASSWORD_HASH = "bench-not-a-real-hash"
BENCH_PROVIDER_NAME = "bench-embedder"

# Default embedding provider: OpenAI text-embedding-3-small (cloud-hosted, cheap,
# avoids TEI container's RAM + CPU overhead on the 4-core/8GB VPS). Override
# anything here via CLI flags.
DEFAULT_PROVIDER_TYPE = "openai"
DEFAULT_EMBEDDING_MODEL = "text-embedding-3-small"
DEFAULT_EMBEDDING_DIMENSIONS = 1536
DEFAULT_EMBEDDING_MAX_MODEL_LEN = 8192


@dataclass(frozen=True)
class PreflightIds:
    user_id: str
    source_ids: dict[str, str]  # source_type -> source_id
    sync_run_ids: dict[str, str]  # source_type -> sync_run_id


@dataclass(frozen=True)
class EmbeddingProviderSpec:
    provider_type: str  # 'openai' | 'jina' | 'cohere' | 'bedrock' | 'local'
    model: str
    dimensions: int
    max_model_len: int
    api_key: str | None = None
    api_url: str | None = None  # only for 'local' / 'jina' / 'cohere'

    def to_db_config(self) -> dict[str, object]:
        """Build the camelCase JSONB config consumed by services/ai/db_config.py."""
        cfg: dict[str, object] = {
            "model": self.model,
            "dimensions": self.dimensions,
            "maxModelLen": self.max_model_len,
        }
        if self.api_key is not None:
            cfg["apiKey"] = self.api_key
        if self.api_url is not None:
            cfg["apiUrl"] = self.api_url
        return cfg


def _ulid() -> str:
    return str(ULID())


def _parse_external_id(filename: str) -> str:
    m = DSID_RE.match(filename)
    if not m:
        raise ValueError(f"filename does not start with dsid_<hex>: {filename}")
    return m.group(1)


def _read_doc(path: Path) -> tuple[str, bytes]:
    # Some bench docs contain literal 0x00 bytes; Postgres TEXT rejects those,
    # so strip them at ingest. The indexer reads content_blobs as BYTEA and
    # casts to documents.content (TEXT) — a single bad byte fails an entire batch.
    raw = path.read_bytes().replace(b"\x00", b"")
    text = raw.decode("utf-8", errors="replace")
    first_newline = text.find("\n")
    title = text[:first_newline].strip() if first_newline >= 0 else text.strip()
    if not title:
        title = path.stem
    return title, raw


def _file_mtime_iso(path: Path) -> str:
    return datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc).isoformat()


def preflight(
    conn: psycopg.Connection,
    source_types: list[str],
    embedding: EmbeddingProviderSpec,
) -> PreflightIds:
    """Idempotently set up the user, sources, sync_runs, and embedding provider rows."""
    with conn.cursor() as cur:
        cur.execute(
            """
            INSERT INTO users (id, email, password_hash, role)
            VALUES (%s, %s, %s, 'admin')
            ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
            RETURNING id
            """,
            (_ulid(), BENCH_USER_EMAIL, BENCH_USER_PASSWORD_HASH),
        )
        user_id = cur.fetchone()[0].strip()

        source_ids: dict[str, str] = {}
        sync_run_ids: dict[str, str] = {}
        for st in source_types:
            cur.execute(
                """
                SELECT id FROM sources WHERE source_type = %s AND name = %s
                """,
                (st, f"bench-{st}"),
            )
            row = cur.fetchone()
            if row:
                source_id = row[0].strip()
            else:
                source_id = _ulid()
                cur.execute(
                    """
                    INSERT INTO sources (id, name, source_type, created_by)
                    VALUES (%s, %s, %s, %s)
                    """,
                    (source_id, f"bench-{st}", st, user_id),
                )
            source_ids[st] = source_id

            cur.execute(
                """
                SELECT id FROM sync_runs
                WHERE source_id = %s AND status = 'running'
                ORDER BY started_at DESC LIMIT 1
                """,
                (source_id,),
            )
            row = cur.fetchone()
            if row:
                sync_run_id = row[0].strip()
            else:
                sync_run_id = _ulid()
                cur.execute(
                    """
                    INSERT INTO sync_runs (id, source_id, sync_type)
                    VALUES (%s, %s, 'full')
                    """,
                    (sync_run_id, source_id),
                )
            sync_run_ids[st] = sync_run_id

        # Stored as plaintext JSON: services/ai/crypto/encryption.py:decrypt_config
        # falls through dicts that lack the "encrypted_data" envelope.
        # Idempotent provider seed: clear current flag, drop any prior row with our
        # name (no unique constraint on name to upsert against), insert fresh.
        cur.execute(
            "UPDATE embedding_providers SET is_current = FALSE WHERE is_current = TRUE"
        )
        cur.execute(
            "DELETE FROM embedding_providers WHERE name = %s", (BENCH_PROVIDER_NAME,)
        )
        cur.execute(
            """
            INSERT INTO embedding_providers (id, name, provider_type, config, is_current)
            VALUES (%s, %s, %s, %s, TRUE)
            """,
            (
                _ulid(),
                BENCH_PROVIDER_NAME,
                embedding.provider_type,
                Jsonb(embedding.to_db_config()),
            ),
        )

    conn.commit()
    log.info("preflight done: user=%s sources=%s", user_id, source_ids)
    return PreflightIds(
        user_id=user_id, source_ids=source_ids, sync_run_ids=sync_run_ids
    )


def load_source(
    conn: psycopg.Connection,
    src_dir: Path,
    source_type: str,
    source_id: str,
    sync_run_id: str,
    batch_size: int,
) -> int:
    files = sorted(p for p in src_dir.rglob("*.txt") if p.is_file())
    if not files:
        log.warning("no .txt files in %s", src_dir)
        return 0

    log.info(
        "loading %d files from %s as source_type=%s", len(files), src_dir, source_type
    )
    inserted = 0
    batch: list[Path] = []

    pbar = tqdm(total=len(files), unit="docs", desc=source_type)
    for fp in files:
        batch.append(fp)
        if len(batch) >= batch_size:
            inserted += _flush_batch(conn, batch, source_type, source_id, sync_run_id)
            pbar.update(len(batch))
            batch = []
    if batch:
        inserted += _flush_batch(conn, batch, source_type, source_id, sync_run_id)
        pbar.update(len(batch))
    pbar.close()
    return inserted


def _flush_batch(
    conn: psycopg.Connection,
    batch: list[Path],
    source_type: str,
    source_id: str,
    sync_run_id: str,
) -> int:
    blob_rows: list[tuple] = []
    event_rows: list[tuple] = []

    for fp in batch:
        external_id = _parse_external_id(fp.name)
        title, content_bytes = _read_doc(fp)
        size_bytes = len(content_bytes)
        sha256 = hashlib.sha256(content_bytes).hexdigest()
        content_id = _ulid()
        blob_rows.append((content_id, content_bytes, "text/plain", size_bytes, sha256))

        payload = {
            "type": "document_created",
            "sync_run_id": sync_run_id,
            "source_id": source_id,
            "document_id": external_id,
            "content_id": content_id,
            "metadata": {
                "title": title,
                "url": f"benchmark://{source_type}/{fp.name}",
                "created_at": _file_mtime_iso(fp),
                "content_type": "text/plain",
            },
            "permissions": {"public": True, "users": [], "groups": []},
        }
        event_rows.append(
            (
                _ulid(),
                sync_run_id,
                source_id,
                "document_created",
                Jsonb(payload),
            )
        )

    with conn.cursor() as cur:
        cur.executemany(
            """
            INSERT INTO content_blobs (id, content, content_type, size_bytes, sha256_hash, storage_backend)
            VALUES (%s, %s, %s, %s, %s, 'postgres')
            """,
            blob_rows,
        )
        cur.executemany(
            """
            INSERT INTO connector_events_queue
                (id, sync_run_id, source_id, event_type, payload)
            VALUES (%s, %s, %s, %s, %s)
            """,
            event_rows,
        )
    conn.commit()
    return len(event_rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--data-dir",
        type=Path,
        required=True,
        help="root containing per-source-type subdirs (e.g. confluence/, jira/)",
    )
    parser.add_argument(
        "--source-types",
        default="confluence,jira",
        help="comma-separated list; each must match a subdir name and a SourceType variant",
    )
    parser.add_argument("--batch-size", type=int, default=1000)
    parser.add_argument(
        "--db-host", default=os.environ.get("BENCH_DB_HOST", "localhost")
    )
    parser.add_argument(
        "--db-port", type=int, default=int(os.environ.get("BENCH_DB_PORT", "5432"))
    )
    parser.add_argument(
        "--db-user", default=os.environ.get("BENCH_DB_USER", "omni_bench")
    )
    parser.add_argument(
        "--db-password",
        default=os.environ.get("BENCH_DB_PASSWORD", "omni_bench_password"),
    )
    parser.add_argument(
        "--db-name", default=os.environ.get("BENCH_DB_NAME", "omni_benchmark")
    )
    parser.add_argument(
        "--embedding-provider",
        default=DEFAULT_PROVIDER_TYPE,
        choices=["openai", "jina", "cohere", "bedrock", "local"],
        help="embedding provider to seed as current in the DB",
    )
    parser.add_argument("--embedding-model", default=DEFAULT_EMBEDDING_MODEL)
    parser.add_argument(
        "--embedding-dimensions", type=int, default=DEFAULT_EMBEDDING_DIMENSIONS
    )
    parser.add_argument(
        "--embedding-max-model-len", type=int, default=DEFAULT_EMBEDDING_MAX_MODEL_LEN
    )
    parser.add_argument(
        "--embedding-api-key",
        default=os.environ.get("OPENAI_API_KEY") or os.environ.get("EMBEDDING_API_KEY"),
        help="API key for the cloud provider (or unset for 'local')",
    )
    parser.add_argument(
        "--embedding-api-url",
        default=os.environ.get("EMBEDDING_API_URL"),
        help="only used for provider=local (TEI URL) or jina/cohere overrides",
    )
    args = parser.parse_args()

    source_types = [s.strip() for s in args.source_types.split(",") if s.strip()]
    if not source_types:
        log.error("no source types specified")
        return 2

    for st in source_types:
        d = args.data_dir / st
        if not d.is_dir():
            log.error("missing source dir: %s", d)
            return 2

    if (
        args.embedding_provider in {"openai", "jina", "cohere"}
        and not args.embedding_api_key
    ):
        log.error(
            "embedding provider %s needs an API key (set OPENAI_API_KEY / EMBEDDING_API_KEY "
            "or pass --embedding-api-key)",
            args.embedding_provider,
        )
        return 2

    embedding = EmbeddingProviderSpec(
        provider_type=args.embedding_provider,
        model=args.embedding_model,
        dimensions=args.embedding_dimensions,
        max_model_len=args.embedding_max_model_len,
        api_key=args.embedding_api_key,
        api_url=args.embedding_api_url,
    )

    conninfo = (
        f"host={args.db_host} port={args.db_port} "
        f"user={args.db_user} password={args.db_password} "
        f"dbname={args.db_name}"
    )
    with psycopg.connect(conninfo) as conn:
        ids = preflight(conn, source_types, embedding)
        total = 0
        for st in source_types:
            total += load_source(
                conn,
                args.data_dir / st,
                st,
                ids.source_ids[st],
                ids.sync_run_ids[st],
                args.batch_size,
            )
        log.info("inserted %d events across %d source(s)", total, len(source_types))
    return 0


if __name__ == "__main__":
    sys.exit(main())
