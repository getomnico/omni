"""Seed model_providers + models with the Kimi K2.6 entry the agentic
benchmark loop needs.

Idempotent: tags rows by name, drops + reinserts on each run.

Reads:
  - KIMI_API_KEY from env (set via /root/EnterpriseRAG-Bench/.env)
  - KIMI_API_URL (defaults to https://api.moonshot.ai/v1)
  - KIMI_MODEL (defaults to kimi-k2.6)

Writes:
  - model_providers row, provider_type='openai_compatible',
    config={apiKey, apiUrl} (plaintext JSON; encryption.py falls through
    when the 'encrypted_data' envelope is missing)
  - models row, model_id=KIMI_MODEL, is_default=true
"""

import argparse
import json
import logging
import os
import sys

import psycopg
import ulid

logging.basicConfig(level=logging.INFO, format="%(levelname)s %(message)s")
log = logging.getLogger("seed_kimi_provider")

PROVIDER_NAME = "moonshot-kimi"
DEFAULT_KIMI_API_URL = "https://api.moonshot.ai/v1"
DEFAULT_KIMI_MODEL = "kimi-k2.6"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db-host", default=os.environ.get("DB_HOST", "localhost"))
    parser.add_argument(
        "--db-port", type=int, default=int(os.environ.get("DB_PORT", "5432"))
    )
    parser.add_argument(
        "--db-name", default=os.environ.get("DB_NAME", "omni_benchmark")
    )
    parser.add_argument("--db-user", default=os.environ.get("DB_USER", "omni_bench"))
    parser.add_argument(
        "--db-password",
        default=os.environ.get("DB_PASSWORD", "omni_bench_password"),
    )
    args = parser.parse_args()

    api_key = os.environ.get("KIMI_API_KEY")
    if not api_key:
        log.error("KIMI_API_KEY not set in env")
        return 2
    api_url = os.environ.get("KIMI_API_URL", DEFAULT_KIMI_API_URL)
    model_id = os.environ.get("KIMI_MODEL", DEFAULT_KIMI_MODEL)

    conn = psycopg.connect(
        host=args.db_host,
        port=args.db_port,
        dbname=args.db_name,
        user=args.db_user,
        password=args.db_password,
        autocommit=False,
    )
    try:
        with conn.cursor() as cur:
            cur.execute("DELETE FROM model_providers WHERE name = %s", (PROVIDER_NAME,))
            provider_id = str(ulid.ULID())
            cur.execute(
                """
                INSERT INTO model_providers (id, name, provider_type, config)
                VALUES (%s, %s, 'openai_compatible', %s::jsonb)
                """,
                (
                    provider_id,
                    PROVIDER_NAME,
                    json.dumps({"apiKey": api_key, "apiUrl": api_url}),
                ),
            )

            # Clear any prior is_default=true (single-default unique partial index
            # would otherwise reject our insert).
            cur.execute("UPDATE models SET is_default = FALSE WHERE is_default = TRUE")
            cur.execute(
                "UPDATE models SET is_secondary = FALSE WHERE is_secondary = TRUE"
            )
            model_row_id = str(ulid.ULID())
            cur.execute(
                """
                INSERT INTO models (
                    id, model_provider_id, model_id, display_name,
                    is_default, is_secondary
                )
                VALUES (%s, %s, %s, %s, TRUE, TRUE)
                """,
                (model_row_id, provider_id, model_id, "Kimi K2.6"),
            )
        conn.commit()
        log.info(
            "seeded provider=%s (id=%s) model=%s (id=%s)",
            PROVIDER_NAME,
            provider_id,
            model_id,
            model_row_id,
        )
    finally:
        conn.close()

    return 0


if __name__ == "__main__":
    sys.exit(main())
