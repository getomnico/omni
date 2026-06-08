"""Base syncer with shared delta query and user iteration logic."""

import abc
import logging
import os
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient

logger = logging.getLogger(__name__)

DEFAULT_MAX_AGE_DAYS = int(os.environ.get("MS_365_MAX_AGE_DAYS", "730"))


class BaseSyncer(abc.ABC):
    """Abstract syncer that iterates over users and runs delta queries."""

    @property
    @abc.abstractmethod
    def name(self) -> str: ...

    @abc.abstractmethod
    async def sync_for_user(
        self,
        client: GraphClient,
        user: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
        delta_tokens: dict[str, str] | None = None,
        token_key: str | None = None,
    ) -> str | None:
        """Sync data for a single user. Returns new delta token or None.

        If `delta_tokens` and `token_key` are provided, implementations can call
        `save_delta_token` with an intermediate resume token after each page so
        progress is durable across interruptions.
        """
        ...

    async def save_delta_token(
        self,
        ctx: SyncContext,
        delta_tokens: dict[str, str],
        token_key: str,
        token: str,
    ) -> None:
        """Persist a delta token while preserving the full token map."""
        delta_tokens[token_key] = token
        await ctx.save_checkpoint({"delta_tokens": delta_tokens})

    async def sync(
        self,
        client: GraphClient,
        ctx: SyncContext,
        state: dict[str, Any],
        source_config: dict[str, Any] | None = None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        """Run sync across all users. Returns updated state dict."""
        source_config = source_config or {}
        delta_tokens: dict[str, str] = state.get("delta_tokens", {})
        # Seed with existing tokens so users not processed this run retain theirs.
        new_tokens: dict[str, str] = dict(delta_tokens)

        users = await client.list_users()
        logger.info("[%s] Syncing across %d users", self.name, len(users))

        users = [
            u
            for u in users
            if ctx.should_index_user(u.get("mail") or u.get("userPrincipalName") or "")
        ]
        logger.info("[%s] %d users after filtering", self.name, len(users))

        attempted = 0
        failed = 0

        for user in users:
            if ctx.is_cancelled():
                logger.info("[%s] Cancelled", self.name)
                return {"delta_tokens": new_tokens}

            user_id = user["id"]
            token = delta_tokens.get(user_id)
            attempted += 1

            try:
                new_token = await self.sync_for_user(
                    client,
                    user,
                    ctx,
                    token,
                    user_cache=user_cache,
                    group_cache=group_cache,
                    delta_tokens=new_tokens,
                    token_key=user_id,
                )
            except Exception as e:
                logger.warning(
                    "[%s] sync_for_user failed for %s: %s",
                    self.name,
                    user.get("displayName", user_id),
                    e,
                )
                failed += 1
                continue

            if new_token:
                await self.save_delta_token(ctx, new_tokens, user_id, new_token)
            else:
                await ctx.save_checkpoint({"delta_tokens": new_tokens})

        if attempted > 0 and failed == attempted:
            raise RuntimeError(f"[{self.name}] sync failed for all {attempted} users")

        return {"delta_tokens": new_tokens}
