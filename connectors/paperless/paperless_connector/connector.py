"""Main PaperlessConnector class."""

import logging
from datetime import datetime, timezone
from typing import Any

from omni_connector import Connector, SyncContext

from .client import AuthenticationError, PaperlessClient, PaperlessError
from .config import CHECKPOINT_INTERVAL
from .mappers import generate_document_content, map_document_to_omni

logger = logging.getLogger(__name__)


class PaperlessConnector(Connector):
    """Paperless-ngx connector for Omni — read-only document indexing."""

    @property
    def name(self) -> str:
        return "paperless_ngx"

    @property
    def display_name(self) -> str:
        return "Paperless-ngx"

    @property
    def version(self) -> str:
        return "1.0.0"

    @property
    def source_types(self) -> list[str]:
        return ["paperless_ngx"]

    @property
    def description(self) -> str:
        return "Index documents and their OCR content from paperless-ngx"

    @property
    def sync_modes(self) -> list[str]:
        return ["full", "incremental"]

    async def sync(
        self,
        source_config: dict[str, Any],
        credentials: dict[str, Any],
        state: dict[str, Any] | None,
        ctx: SyncContext,
    ) -> None:
        base_url = source_config.get("base_url", "").strip().rstrip("/")
        if not base_url:
            await ctx.fail("Missing 'base_url' in source config")
            return

        api_key = credentials.get("api_key", "").strip()
        if not api_key:
            await ctx.fail("Missing 'api_key' in credentials")
            return

        client = PaperlessClient(base_url=base_url, api_key=api_key)

        try:
            try:
                await client.validate()
            except AuthenticationError as e:
                await ctx.fail(f"Authentication failed: {e}")
                return
            except PaperlessError as e:
                await ctx.fail(f"Connection to paperless-ngx failed: {e}")
                return

            # Incremental sync: if we have state with a last_sync_at timestamp,
            # only fetch documents modified after that time. This is state-driven
            # (like the ClickUp connector) rather than relying on ctx.sync_mode,
            # because SyncContext does not expose sync_mode.
            modified_after: datetime | None = None
            if state:
                last_sync_ts = state.get("last_sync_at")
                if last_sync_ts:
                    try:
                        modified_after = datetime.fromisoformat(last_sync_ts)
                    except ValueError:
                        logger.warning("Could not parse last_sync_at: %r", last_sync_ts)

            sync_started_at = datetime.now(tz=timezone.utc)

            logger.info(
                "Fetching paperless-ngx documents (modified_after=%s)",
                modified_after,
            )

            try:
                raw_docs = await client.list_documents(modified_after=modified_after)
            except AuthenticationError as e:
                await ctx.fail(f"Authentication failed during document listing: {e}")
                return
            except PaperlessError as e:
                await ctx.fail(f"Failed to list documents: {e}")
                return

            logger.info("Found %d documents to process", len(raw_docs))
            docs_since_checkpoint = 0

            for raw in raw_docs:
                if ctx.is_cancelled():
                    await ctx.fail("Cancelled by user")
                    return

                await ctx.increment_scanned()
                doc_id = raw.get("id", "?")

                try:
                    doc = await client.parse_document(raw)
                    content = generate_document_content(doc)
                    content_id = await ctx.content_storage.save(content, "text/plain")
                    omni_doc = map_document_to_omni(doc, content_id, ctx.source_id, base_url)
                    await ctx.emit(omni_doc)
                    docs_since_checkpoint += 1
                except Exception as e:
                    eid = f"paperless:{ctx.source_id}:{doc_id}"
                    logger.warning("Error processing document %s: %s", eid, e)
                    await ctx.emit_error(eid, str(e))
                    continue

                if docs_since_checkpoint >= CHECKPOINT_INTERVAL:
                    await ctx.save_state(
                        {"last_sync_at": sync_started_at.isoformat()}
                    )
                    docs_since_checkpoint = 0

            await ctx.complete(
                new_state={"last_sync_at": sync_started_at.isoformat()}
            )
            logger.info(
                "Sync completed: %d scanned, %d emitted",
                ctx.documents_scanned,
                ctx.documents_emitted,
            )

        except AuthenticationError as e:
            logger.error("Authentication error during sync: %s", e)
            await ctx.fail(f"Authentication failed: {e}")
        except Exception as e:
            logger.exception("Sync failed with unexpected error")
            await ctx.fail(str(e))
        finally:
            await client.close()
