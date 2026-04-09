"""Microsoft Teams syncer: channel messages and chats (1:1, group, meeting)."""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone
from typing import Any

from omni_connector import SyncContext
from omni_connector.models import DocumentPermissions

from ..graph_client import GraphAPIError, GraphClient
from ..mappers import (
    generate_drive_item_content,
    generate_teams_message_content,
    map_drive_item_to_document,
    map_teams_chat_messages_to_document,
    map_teams_messages_to_document,
    strip_html,
    _parse_iso,
)
from .base import DEFAULT_MAX_AGE_DAYS
from .onedrive import (
    INDEXABLE_EXTENSIONS,
    INDEXABLE_MIME_PREFIXES,
    _get_extension,
    _is_indexable,
)

logger = logging.getLogger(__name__)

MAX_MESSAGES_PER_GROUP = 100
MAX_CONTENT_BYTES_PER_GROUP = 50_000


@dataclass
class TeamsMessageGroup:
    """A group of Teams messages forming a single indexable document."""

    team_id: str
    team_name: str
    channel_id: str
    channel_name: str
    date: date
    is_thread: bool
    root_message_id: str | None = None
    part: int = 0
    permissions: DocumentPermissions | None = None
    messages: list[tuple[dict[str, Any], str]] = field(default_factory=list)

    @property
    def external_id(self) -> str:
        if self.is_thread:
            return (
                f"teams:{self.team_id}:{self.channel_id}:thread:{self.root_message_id}"
            )
        ext_id = f"teams:{self.team_id}:{self.channel_id}:{self.date}"
        if self.part > 0:
            ext_id += f"_p{self.part}"
        return ext_id

    @property
    def message_count(self) -> int:
        return len(self.messages)

    @property
    def authors(self) -> list[str]:
        return [sender for _, sender in self.messages]

    @property
    def first_timestamp(self) -> datetime | None:
        if not self.messages:
            return None
        return _parse_iso(self.messages[0][0].get("createdDateTime"))

    @property
    def last_timestamp(self) -> datetime | None:
        if not self.messages:
            return None
        return _parse_iso(self.messages[-1][0].get("createdDateTime"))

    @property
    def content_size(self) -> int:
        total = 0
        for msg, _ in self.messages:
            body = msg.get("body", {})
            total += len(body.get("content", ""))
        return total

    def should_split(self) -> bool:
        return (
            self.message_count >= MAX_MESSAGES_PER_GROUP
            or self.content_size >= MAX_CONTENT_BYTES_PER_GROUP
        )


def _get_sender_name(msg: dict[str, Any], user_cache: dict[str, str]) -> str:
    """Extract display name of the message sender."""
    from_field = msg.get("from") or {}
    user = from_field.get("user") or {}
    name = user.get("displayName")
    if name:
        return name
    user_id = user.get("id", "")
    if user_id and user_id in user_cache:
        return user_cache[user_id]
    return user.get("id") or "Unknown"


def _message_date(msg: dict[str, Any]) -> date:
    """Extract date from a message's createdDateTime."""
    dt = _parse_iso(msg.get("createdDateTime"))
    if dt:
        return dt.date()
    return date.today()


def group_channel_messages(
    messages: list[dict[str, Any]],
    replies_by_msg: dict[str, list[dict[str, Any]]],
    team_id: str,
    team_name: str,
    channel_id: str,
    channel_name: str,
    user_cache: dict[str, str],
    permissions: DocumentPermissions,
) -> list[TeamsMessageGroup]:
    """Group messages by date and thread, splitting large groups."""
    thread_groups: list[TeamsMessageGroup] = []
    daily_messages: dict[date, list[tuple[dict[str, Any], str]]] = {}

    for msg in messages:
        msg_id = msg.get("id", "")
        sender = _get_sender_name(msg, user_cache)
        replies = replies_by_msg.get(msg_id, [])

        if replies:
            # Thread: root message + replies become one document
            group = TeamsMessageGroup(
                team_id=team_id,
                team_name=team_name,
                channel_id=channel_id,
                channel_name=channel_name,
                date=_message_date(msg),
                is_thread=True,
                root_message_id=msg_id,
                permissions=permissions,
            )
            group.messages.append((msg, sender))
            for reply in sorted(replies, key=lambda r: r.get("createdDateTime", "")):
                reply_sender = _get_sender_name(reply, user_cache)
                group.messages.append((reply, reply_sender))
            thread_groups.append(group)
        else:
            # Non-thread: group by date
            msg_date = _message_date(msg)
            if msg_date not in daily_messages:
                daily_messages[msg_date] = []
            daily_messages[msg_date].append((msg, sender))

    # Build daily groups, splitting if too large
    daily_groups: list[TeamsMessageGroup] = []
    for msg_date in sorted(daily_messages.keys()):
        msgs = daily_messages[msg_date]
        part = 0
        current = TeamsMessageGroup(
            team_id=team_id,
            team_name=team_name,
            channel_id=channel_id,
            channel_name=channel_name,
            date=msg_date,
            is_thread=False,
            part=part,
            permissions=permissions,
        )
        for msg_tuple in msgs:
            current.messages.append(msg_tuple)
            if current.should_split():
                daily_groups.append(current)
                part += 1
                current = TeamsMessageGroup(
                    team_id=team_id,
                    team_name=team_name,
                    channel_id=channel_id,
                    channel_name=channel_name,
                    date=msg_date,
                    is_thread=False,
                    part=part,
                    permissions=permissions,
                )
        if current.messages:
            daily_groups.append(current)

    return daily_groups + thread_groups


@dataclass
class TeamsChatMessageGroup:
    """A group of Teams chat messages forming a single indexable document."""

    chat_id: str
    chat_type: str
    chat_topic: str | None
    participant_names: list[str]
    date: date
    part: int = 0
    permissions: DocumentPermissions | None = None
    messages: list[tuple[dict[str, Any], str]] = field(default_factory=list)

    @property
    def external_id(self) -> str:
        ext_id = f"teams_chat:{self.chat_id}:{self.date}"
        if self.part > 0:
            ext_id += f"_p{self.part}"
        return ext_id

    @property
    def message_count(self) -> int:
        return len(self.messages)

    @property
    def authors(self) -> list[str]:
        return [sender for _, sender in self.messages]

    @property
    def first_timestamp(self) -> datetime | None:
        if not self.messages:
            return None
        return _parse_iso(self.messages[0][0].get("createdDateTime"))

    @property
    def last_timestamp(self) -> datetime | None:
        if not self.messages:
            return None
        return _parse_iso(self.messages[-1][0].get("createdDateTime"))

    @property
    def content_size(self) -> int:
        total = 0
        for msg, _ in self.messages:
            body = msg.get("body", {})
            total += len(body.get("content", ""))
        return total

    def should_split(self) -> bool:
        return (
            self.message_count >= MAX_MESSAGES_PER_GROUP
            or self.content_size >= MAX_CONTENT_BYTES_PER_GROUP
        )


def group_chat_messages(
    messages: list[dict[str, Any]],
    chat_id: str,
    chat_type: str,
    chat_topic: str | None,
    participant_names: list[str],
    user_cache: dict[str, str],
    permissions: DocumentPermissions,
) -> list[TeamsChatMessageGroup]:
    """Group chat messages by date, splitting large groups."""
    daily_messages: dict[date, list[tuple[dict[str, Any], str]]] = {}

    for msg in messages:
        sender = _get_sender_name(msg, user_cache)
        msg_date = _message_date(msg)
        if msg_date not in daily_messages:
            daily_messages[msg_date] = []
        daily_messages[msg_date].append((msg, sender))

    groups: list[TeamsChatMessageGroup] = []
    for msg_date in sorted(daily_messages.keys()):
        msgs = daily_messages[msg_date]
        part = 0
        current = TeamsChatMessageGroup(
            chat_id=chat_id,
            chat_type=chat_type,
            chat_topic=chat_topic,
            participant_names=participant_names,
            date=msg_date,
            part=part,
            permissions=permissions,
        )
        for msg_tuple in msgs:
            current.messages.append(msg_tuple)
            if current.should_split():
                groups.append(current)
                part += 1
                current = TeamsChatMessageGroup(
                    chat_id=chat_id,
                    chat_type=chat_type,
                    chat_topic=chat_topic,
                    participant_names=participant_names,
                    date=msg_date,
                    part=part,
                    permissions=permissions,
                )
        if current.messages:
            groups.append(current)

    return groups


class TeamsSyncer:
    """Syncs Microsoft Teams: channel messages and chats.

    Phase 1 — Channels: iterates teams, channels, and messages using delta queries.
    Phase 2 — Chats: iterates users, their 1:1/group/meeting chats, and messages.
    """

    @property
    def name(self) -> str:
        return "teams"

    async def sync(
        self,
        client: GraphClient,
        ctx: SyncContext,
        state: dict[str, Any],
        source_config: dict[str, Any] | None = None,
        user_cache: dict[str, str] | None = None,
        group_cache: dict[str, str] | None = None,
    ) -> dict[str, Any]:
        source_config = source_config or {}
        user_cache = user_cache or {}
        group_cache = group_cache or {}
        delta_tokens: dict[str, str] = state.get("delta_tokens", {})
        last_sync_ts: dict[str, str] = state.get("last_sync_ts", {})
        chat_last_sync_ts: dict[str, str] = state.get("chat_last_sync_ts", {})
        new_tokens: dict[str, str] = {}
        new_sync_ts: dict[str, str] = {}
        new_chat_sync_ts: dict[str, str] = {}

        cutoff = datetime.now(timezone.utc) - timedelta(days=DEFAULT_MAX_AGE_DAYS)
        now_iso = datetime.now(timezone.utc).isoformat()

        # Phase 1: Channel messages
        teams = await self._list_teams(client)
        logger.info("[teams] Found %d teams", len(teams))

        team_members_cache: dict[str, list[str]] = {}

        for team in teams:
            if ctx.is_cancelled():
                return state

            team_id = team["id"]
            team_name = team.get("displayName", team_id)

            try:
                channels = await client.list_team_channels(team_id)
            except GraphAPIError as e:
                logger.warning(
                    "[teams] Failed to list channels for team %s: %s", team_name, e
                )
                continue

            logger.info("[teams] Team %s has %d channels", team_name, len(channels))

            for channel in channels:
                if ctx.is_cancelled():
                    return state

                channel_id = channel["id"]
                channel_name = channel.get("displayName", channel_id)
                token_key = f"{team_id}:{channel_id}"
                delta_token = delta_tokens.get(token_key)
                channel_last_sync = last_sync_ts.get(token_key)

                try:
                    permissions = await self._resolve_channel_permissions(
                        client, team, channel, user_cache, team_members_cache
                    )
                except Exception as e:
                    logger.warning(
                        "[teams] Failed to resolve permissions for %s/%s: %s",
                        team_name,
                        channel_name,
                        e,
                    )
                    permissions = DocumentPermissions(public=False)

                new_token = await self._sync_channel(
                    client=client,
                    team=team,
                    channel=channel,
                    ctx=ctx,
                    delta_token=delta_token,
                    cutoff=cutoff,
                    user_cache=user_cache,
                    permissions=permissions,
                    last_sync_ts=channel_last_sync,
                )
                if new_token:
                    new_tokens[token_key] = new_token
                    new_sync_ts[token_key] = now_iso

                await ctx.save_state(
                    {
                        "delta_tokens": new_tokens,
                        "last_sync_ts": new_sync_ts,
                        "chat_last_sync_ts": chat_last_sync_ts,
                    }
                )

        # Phase 2: Chats (1:1, group, meeting)
        new_chat_sync_ts = await self._sync_chats(
            client=client,
            ctx=ctx,
            cutoff=cutoff,
            now_iso=now_iso,
            user_cache=user_cache,
            chat_last_sync_ts=chat_last_sync_ts,
            channel_state={"delta_tokens": new_tokens, "last_sync_ts": new_sync_ts},
        )

        return {
            "delta_tokens": new_tokens,
            "last_sync_ts": new_sync_ts,
            "chat_last_sync_ts": new_chat_sync_ts,
        }

    async def _list_teams(self, client: GraphClient) -> list[dict[str, Any]]:
        try:
            return await client.list_teams()
        except GraphAPIError as e:
            logger.error("[teams] Failed to list teams: %s", e)
            return []

    async def _sync_channel(
        self,
        client: GraphClient,
        team: dict[str, Any],
        channel: dict[str, Any],
        ctx: SyncContext,
        delta_token: str | None,
        cutoff: datetime,
        user_cache: dict[str, str],
        permissions: DocumentPermissions,
        last_sync_ts: str | None = None,
    ) -> str | None:
        """Sync a single channel's messages.

        Two-phase incremental approach:
        1. Call delta with stored token to detect changes and get new token
        2. If changes exist and this is incremental (has last_sync_ts):
           re-fetch all messages from start-of-day(last_sync_ts) to get
           complete daily/thread docs, not just the changed fragments
        """
        team_id = team["id"]
        team_name = team.get("displayName", team_id)
        channel_id = channel["id"]
        channel_name = channel.get("displayName", channel_id)
        is_first_sync = delta_token is None

        logger.info("[teams] Syncing channel %s/%s", team_name, channel_name)

        # Phase 1: detect changes via delta
        use_delta = True
        try:
            delta_items, new_token = await client.get_channel_messages_delta(
                team_id, channel_id, delta_token
            )
        except GraphAPIError as e:
            if e.status_code == 410:
                logger.warning(
                    "[teams] Delta token expired for %s/%s, doing full sync",
                    team_name,
                    channel_name,
                )
                try:
                    delta_items, new_token = await client.get_channel_messages_delta(
                        team_id, channel_id, None
                    )
                    is_first_sync = True
                except GraphAPIError as e2:
                    logger.warning(
                        "[teams] Full sync fallback failed for %s/%s: %s",
                        team_name,
                        channel_name,
                        e2,
                    )
                    return delta_token
            elif e.status_code == 400 and "DeltaToken" in str(e):
                # Some channels (e.g. certain private channels) don't support
                # delta queries — fall back to listing messages directly
                logger.info(
                    "[teams] Delta not supported for %s/%s, using list fallback",
                    team_name,
                    channel_name,
                )
                use_delta = False
                try:
                    delta_items = await client.list_channel_messages(
                        team_id, channel_id
                    )
                    new_token = None
                    is_first_sync = True
                except GraphAPIError as e2:
                    logger.warning(
                        "[teams] List fallback failed for %s/%s: %s",
                        team_name,
                        channel_name,
                        e2,
                    )
                    return delta_token
            else:
                logger.warning(
                    "[teams] Failed to fetch messages for %s/%s: %s",
                    team_name,
                    channel_name,
                    e,
                )
                return delta_token

        # No changes detected — nothing to do
        if not delta_items:
            return new_token

        # Phase 2: determine which messages to process
        if is_first_sync:
            # First sync: use the delta items directly
            messages = delta_items
        elif last_sync_ts:
            # Incremental sync: re-fetch from start of the day of last sync
            # to get complete daily/thread documents
            resync_from = self._start_of_day_iso(last_sync_ts)
            logger.info(
                "[teams] Incremental: re-fetching messages from %s for %s/%s",
                resync_from,
                team_name,
                channel_name,
            )
            try:
                messages, _ = await client.get_channel_messages_delta(
                    team_id, channel_id, None, filter_from=resync_from
                )
            except GraphAPIError as e:
                logger.warning(
                    "[teams] Failed to re-fetch messages for %s/%s: %s",
                    team_name,
                    channel_name,
                    e,
                )
                # Fall back to using the delta items
                messages = delta_items
        else:
            # Has a delta token but no last_sync_ts (shouldn't happen normally,
            # but handle gracefully by using delta items)
            messages = delta_items

        # Filter to real top-level messages only
        top_level_messages = [
            m
            for m in messages
            if m.get("messageType") == "message" and not m.get("replyToId")
        ]

        # Apply time cutoff on first sync
        if is_first_sync:
            top_level_messages = [
                m for m in top_level_messages if not self._is_before_cutoff(m, cutoff)
            ]

        if not top_level_messages:
            return new_token

        # Fetch replies for top-level messages
        replies_by_msg: dict[str, list[dict[str, Any]]] = {}
        for msg in top_level_messages:
            msg_id = msg.get("id", "")
            try:
                replies = await client.list_message_replies(team_id, channel_id, msg_id)
                replies = [r for r in replies if r.get("messageType") == "message"]
                if replies:
                    replies_by_msg[msg_id] = replies
            except GraphAPIError as e:
                logger.warning(
                    "[teams] Failed to fetch replies for message %s: %s",
                    msg_id,
                    e,
                )

        # Group messages
        groups = group_channel_messages(
            messages=top_level_messages,
            replies_by_msg=replies_by_msg,
            team_id=team_id,
            team_name=team_name,
            channel_id=channel_id,
            channel_name=channel_name,
            user_cache=user_cache,
            permissions=permissions,
        )

        # Emit message documents (upserts on incremental since external IDs match)
        for group in groups:
            if ctx.is_cancelled():
                return delta_token

            await ctx.increment_scanned()

            try:
                content = generate_teams_message_content(group.messages)
                content_id = await ctx.content_storage.save(content, "text/plain")
                doc = map_teams_messages_to_document(group, content_id)
                await ctx.emit(doc)
            except Exception as e:
                logger.warning(
                    "[teams] Error emitting message group %s: %s",
                    group.external_id,
                    e,
                )

        # Process file attachments from all messages (top-level + replies)
        all_messages = list(top_level_messages)
        for replies in replies_by_msg.values():
            all_messages.extend(replies)

        await self._process_file_attachments(
            client=client,
            messages=all_messages,
            ctx=ctx,
            permissions=permissions,
            user_cache=user_cache,
        )

        return new_token

    @staticmethod
    def _start_of_day_iso(iso_ts: str) -> str:
        """Truncate an ISO timestamp to the start of its day."""
        dt = _parse_iso(iso_ts)
        if dt is None:
            return iso_ts
        start = dt.replace(hour=0, minute=0, second=0, microsecond=0)
        return start.isoformat()

    async def _process_file_attachments(
        self,
        client: GraphClient,
        messages: list[dict[str, Any]],
        ctx: SyncContext,
        permissions: DocumentPermissions,
        user_cache: dict[str, str],
    ) -> None:
        """Extract and index file attachments from messages."""
        seen_urls: set[str] = set()

        for msg in messages:
            for attachment in msg.get("attachments") or []:
                if attachment.get("contentType") != "reference":
                    continue

                content_url = attachment.get("contentUrl")
                if not content_url or content_url in seen_urls:
                    continue
                seen_urls.add(content_url)

                try:
                    await self._process_single_attachment(
                        client=client,
                        content_url=content_url,
                        attachment_name=attachment.get("name", ""),
                        ctx=ctx,
                        permissions=permissions,
                        user_cache=user_cache,
                    )
                except Exception as e:
                    logger.warning(
                        "[teams] Failed to process attachment %s: %s",
                        attachment.get("name", content_url),
                        e,
                    )

    async def _process_single_attachment(
        self,
        client: GraphClient,
        content_url: str,
        attachment_name: str,
        ctx: SyncContext,
        permissions: DocumentPermissions,
        user_cache: dict[str, str],
    ) -> None:
        """Resolve a file attachment via the Shares API and index it."""
        drive_item = await client.resolve_share(content_url)

        parent_ref = drive_item.get("parentReference", {})
        site_id = parent_ref.get("siteId")
        drive_id = parent_ref.get("driveId")
        item_id = drive_item.get("id")

        if not item_id or not drive_id:
            logger.warning(
                "[teams] Resolved share missing driveId/itemId for %s", content_url
            )
            return

        file_info = drive_item.get("file", {})
        mime_type = file_info.get("mimeType", "")
        file_name = drive_item.get("name", attachment_name)
        extension = _get_extension(file_name)

        if _is_indexable(mime_type, extension):
            try:
                data = await client.get_binary(
                    f"/drives/{drive_id}/items/{item_id}/content"
                )
                content_id = await ctx.content_storage.extract_and_store_content(
                    data, mime_type, file_name
                )
            except Exception as e:
                logger.warning(
                    "[teams] Failed to extract content for attachment %s: %s",
                    file_name,
                    e,
                )
                content = generate_drive_item_content(drive_item, {})
                content_id = await ctx.content_storage.save(content, "text/plain")
        else:
            content = generate_drive_item_content(drive_item, {})
            content_id = await ctx.content_storage.save(content, "text/plain")

        # Use sharepoint: external ID for dedup with SharePoint syncer
        if site_id:
            external_id_prefix_site = site_id
        else:
            external_id_prefix_site = drive_id

        doc = map_drive_item_to_document(
            item=drive_item,
            content_id=content_id,
            source_type="share_point",
            graph_permissions=[],
            user_cache=user_cache,
            site_id=external_id_prefix_site,
        )
        # Override permissions with channel permissions
        doc.permissions = permissions
        await ctx.emit(doc)

    async def _resolve_channel_permissions(
        self,
        client: GraphClient,
        team: dict[str, Any],
        channel: dict[str, Any],
        user_cache: dict[str, str],
        team_members_cache: dict[str, list[str]],
    ) -> DocumentPermissions:
        """Resolve channel members to email addresses for permissions."""
        membership_type = channel.get("membershipType", "standard")
        team_id = team["id"]

        if membership_type == "standard":
            # Standard channels inherit team membership
            if team_id not in team_members_cache:
                try:
                    members = await client.list_group_members(team_id)
                    emails = [
                        (m.get("mail") or m.get("userPrincipalName") or "").lower()
                        for m in members
                    ]
                    team_members_cache[team_id] = sorted(set(e for e in emails if e))
                except Exception as e:
                    logger.warning(
                        "[teams] Failed to list team members for %s: %s",
                        team_id,
                        e,
                    )
                    team_members_cache[team_id] = []

            return DocumentPermissions(
                public=False,
                users=team_members_cache[team_id],
            )
        else:
            # Private or shared channels have explicit members
            channel_id = channel["id"]
            try:
                members = await client.list_channel_members(team_id, channel_id)
                emails = []
                for m in members:
                    email = m.get("email")
                    if email:
                        emails.append(email.lower())
                    else:
                        user_id = m.get("userId", "")
                        if user_id in user_cache:
                            emails.append(user_cache[user_id].lower())
                return DocumentPermissions(
                    public=False,
                    users=sorted(set(emails)),
                )
            except Exception as e:
                logger.warning(
                    "[teams] Failed to list channel members for %s/%s: %s",
                    team_id,
                    channel_id,
                    e,
                )
                return DocumentPermissions(public=False)

    @staticmethod
    def _is_before_cutoff(msg: dict[str, Any], cutoff: datetime) -> bool:
        dt = _parse_iso(msg.get("createdDateTime"))
        if dt is None:
            return False
        return dt < cutoff

    # ── Chat sync (Phase 2) ──────────────────────────────────────────

    async def _sync_chats(
        self,
        client: GraphClient,
        ctx: SyncContext,
        cutoff: datetime,
        now_iso: str,
        user_cache: dict[str, str],
        chat_last_sync_ts: dict[str, str],
        channel_state: dict[str, Any] | None = None,
    ) -> dict[str, str]:
        """Iterate users, collect their chats, and sync messages."""
        new_chat_sync_ts: dict[str, str] = {}

        try:
            users = await client.list_users()
        except Exception as e:
            logger.warning("[teams] Failed to list users for chat sync: %s", e)
            return chat_last_sync_ts

        users = [
            u
            for u in users
            if ctx.should_index_user(u.get("mail") or u.get("userPrincipalName") or "")
        ]
        logger.info("[teams] Syncing chats across %d users", len(users))

        seen_chat_ids: set[str] = set()

        for user in users:
            if ctx.is_cancelled():
                return chat_last_sync_ts

            user_id = user["id"]
            user_email = (
                user.get("mail") or user.get("userPrincipalName") or ""
            ).lower()

            try:
                chats = await client.list_user_chats(user_id)
            except GraphAPIError as e:
                logger.warning(
                    "[teams] Failed to list chats for user %s: %s", user_email, e
                )
                continue

            for chat in chats:
                if ctx.is_cancelled():
                    return chat_last_sync_ts

                chat_id = chat.get("id", "")
                if not chat_id or chat_id in seen_chat_ids:
                    continue
                seen_chat_ids.add(chat_id)

                # Skip unchanged chats on incremental sync
                chat_last_updated = chat.get("lastUpdatedDateTime")
                prev_sync = chat_last_sync_ts.get(chat_id)
                if prev_sync and chat_last_updated:
                    prev_dt = _parse_iso(prev_sync)
                    updated_dt = _parse_iso(chat_last_updated)
                    if prev_dt and updated_dt and updated_dt <= prev_dt:
                        new_chat_sync_ts[chat_id] = prev_sync
                        continue

                permissions = self._resolve_chat_permissions(chat, user_cache)
                participant_names = self._get_participant_names(chat)

                ok = await self._sync_chat(
                    client=client,
                    chat=chat,
                    ctx=ctx,
                    cutoff=cutoff,
                    user_cache=user_cache,
                    permissions=permissions,
                    participant_names=participant_names,
                    last_sync_ts=prev_sync,
                )
                if ok:
                    new_chat_sync_ts[chat_id] = now_iso

                await ctx.save_state(
                    {
                        **(channel_state or {}),
                        "chat_last_sync_ts": new_chat_sync_ts,
                    }
                )

        logger.info("[teams] Processed %d chats", len(seen_chat_ids))
        return new_chat_sync_ts

    async def _sync_chat(
        self,
        client: GraphClient,
        chat: dict[str, Any],
        ctx: SyncContext,
        cutoff: datetime,
        user_cache: dict[str, str],
        permissions: DocumentPermissions,
        participant_names: list[str],
        last_sync_ts: str | None = None,
    ) -> bool:
        """Sync a single chat's messages. Returns True if successful."""
        chat_id = chat["id"]
        chat_type = chat.get("chatType", "oneOnOne")
        chat_topic = chat.get("topic")
        is_first_sync = last_sync_ts is None

        filter_from = None
        if not is_first_sync and last_sync_ts:
            filter_from = self._start_of_day_iso(last_sync_ts)

        try:
            messages = await client.list_chat_messages(chat_id, filter_from=filter_from)
        except GraphAPIError as e:
            logger.warning(
                "[teams] Failed to fetch messages for chat %s: %s", chat_id, e
            )
            return False

        # Filter to real messages, skip system events and deleted
        messages = [
            m
            for m in messages
            if m.get("messageType") == "message" and not m.get("deletedDateTime")
        ]

        if is_first_sync:
            messages = [m for m in messages if not self._is_before_cutoff(m, cutoff)]

        if not messages:
            return True

        # Sort chronologically for grouping
        messages.sort(key=lambda m: m.get("createdDateTime", ""))

        groups = group_chat_messages(
            messages=messages,
            chat_id=chat_id,
            chat_type=chat_type,
            chat_topic=chat_topic,
            participant_names=participant_names,
            user_cache=user_cache,
            permissions=permissions,
        )

        for group in groups:
            if ctx.is_cancelled():
                return False

            await ctx.increment_scanned()

            try:
                content = generate_teams_message_content(group.messages)
                content_id = await ctx.content_storage.save(content, "text/plain")
                doc = map_teams_chat_messages_to_document(group, content_id)
                await ctx.emit(doc)
            except Exception as e:
                logger.warning(
                    "[teams] Error emitting chat message group %s: %s",
                    group.external_id,
                    e,
                )

        # Process file attachments
        await self._process_file_attachments(
            client=client,
            messages=messages,
            ctx=ctx,
            permissions=permissions,
            user_cache=user_cache,
        )

        return True

    @staticmethod
    def _resolve_chat_permissions(
        chat: dict[str, Any],
        user_cache: dict[str, str],
    ) -> DocumentPermissions:
        """Resolve chat members to email addresses."""
        members = chat.get("members") or []
        emails: list[str] = []
        for member in members:
            email = member.get("email")
            if email:
                emails.append(email.lower())
            else:
                user_id = member.get("userId", "")
                if user_id and user_id in user_cache:
                    emails.append(user_cache[user_id].lower())
        return DocumentPermissions(
            public=False,
            users=sorted(set(emails)),
        )

    @staticmethod
    def _get_participant_names(chat: dict[str, Any]) -> list[str]:
        """Extract display names of chat participants."""
        members = chat.get("members") or []
        return [m["displayName"] for m in members if m.get("displayName")]
