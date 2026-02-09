"""Base syncer with shared delta query and user iteration logic."""

import abc
import logging
from typing import Any

from omni_connector import SyncContext

from ..graph_client import GraphClient

logger = logging.getLogger(__name__)


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
    ) -> str | None:
        """Sync data for a single user. Returns new delta token or None."""
        ...

    async def sync(
        self,
        client: GraphClient,
        ctx: SyncContext,
        state: dict[str, Any],
    ) -> dict[str, Any]:
        """Run sync across all users. Returns updated state dict."""
        delta_tokens: dict[str, str] = state.get(f"{self.name}_delta_tokens", {})
        new_tokens: dict[str, str] = {}

        users = await client.list_users()
        logger.info("[%s] Syncing across %d users", self.name, len(users))

        for user in users:
            if ctx.is_cancelled():
                logger.info("[%s] Cancelled", self.name)
                return state

            user_id = user["id"]
            token = delta_tokens.get(user_id)

            new_token = await self.sync_for_user(client, user, ctx, token)
            if new_token:
                new_tokens[user_id] = new_token

        return {f"{self.name}_delta_tokens": new_tokens}
