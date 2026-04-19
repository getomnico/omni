# Telegram Connector for Omni

A connector that syncs Telegram chats, groups, and channels into Omni. Supports two backends with different trade-offs:

| Backend | Coverage | Auth | Use case |
|---------|----------|------|----------|
| **Telethon (MTProto)** | Full chat history | User session string | Index existing conversations, groups, channels |
| **Bot API** | Forward-only (messages received after setup) | Bot token | Passive capture from new messages |

The backend is auto-detected from the credentials shape — no explicit mode switch needed.

## Supported content

- **Chats** — metadata for each chat/group/channel the user is part of
- **Messages** — text messages, captions, replies, forwards, poll questions
- **Media references** — photos, documents, videos, audio, stickers (metadata only; file contents not downloaded)

## Configuration

### Telethon mode (recommended — full history)

**Credentials:**

```json
{
  "api_id": "12345678",
  "api_hash": "a1b2c3d4e5f6...",
  "session": "1BVtsO..."
}
```

**How to obtain:**

1. Go to https://my.telegram.org/apps and create an app. Note the `api_id` and `api_hash`.
2. Sign in to Telegram. You can do this two ways:

   **Option A — via the Omni admin UI (recommended):** open "Connect Telegram" in the integrations settings, pick *Sign in with phone number*, paste your `api_id`/`api_hash`, enter your phone number, then the SMS login code, then (if enabled) your 2FA password. Omni stores the resulting session string automatically — no file copy-paste, no shell script.

   **Option B — via the CLI (useful for CI / air-gapped runs):**

   ```bash
   python connectors/telegram/scripts/auth.py \
     --api-id YOUR_API_ID \
     --api-hash YOUR_API_HASH
   ```

   The script prompts for the phone number, SMS code, and 2FA password if enabled. It prints a session string at the end. In the Omni UI, pick *Paste existing session string* and paste the value.

3. Either path stores the three values (`api_id`, `api_hash`, `session`) as credentials for a new Telegram source. You can also POST directly to `/api/service-credentials`.

**Security note:** The session string grants full access to the Telegram account. It does not expire automatically. Rotate periodically and audit active sessions via Telegram Settings → Devices.

### Bot API mode (forward-only)

**Credentials:**

```json
{
  "token": "123456:ABC-DEF..."
}
```

**How to obtain:** message [@BotFather](https://t.me/BotFather) in Telegram to create a bot and copy the token.

**Limitations:** Bot API can only read messages sent *after* the bot is added to a chat. It cannot access historical messages or discover existing groups. `list_chats` is not supported.

## Source configuration

The `sources.config` JSONB can include:

| Field | Type | Description |
|-------|------|-------------|
| `chats` | `string[]` | Chat titles to sync (matched case-insensitively). If empty and `chat_ids` is empty, syncs all dialogs. |
| `chat_ids` | `number[]` | Chat IDs to sync. Useful when multiple chats share a title. Can be mixed with `chats`. |
| `allowed_users` | `string[]` | **Optional.** Emails of users allowed to view synced documents. If set, emitted documents have `permissions: { public: false, users: [...] }` instead of the default `public: true` for groups/channels. Use this for sensitive sources (e.g. BD conversations) that should only be visible to specific users/agents. |

Example:

```json
{
  "chat_ids": [-1001234567890, -1009876543210],
  "allowed_users": ["admin@example.com"]
}
```

## Actions

### `list_chats`

Returns all chats the authenticated user has access to, with `id`, `title`, `type`, `username`, `unread_count`, and `message_count`. Telethon only — Bot API tokens cannot list chats.

```json
{
  "action": "list_chats",
  "params": {}
}
```

Response:

```json
{
  "chats": [
    {
      "id": -1001234567890,
      "title": "Compute Labs <> Partner",
      "type": "supergroup",
      "unread_count": 3,
      "message_count": null
    }
  ],
  "count": 1
}
```

## Incremental sync

The connector checkpoints per-chat via `chat_last_message_ids[chat_id] = max_message_id` in the connector state. On re-sync, it fetches only messages with `id > last_seen` for each chat, plus a fresh chat metadata document.

## Development

```bash
cd connectors/telegram
uv sync
uv run python main.py
```

The connector runs as an HTTP service on `PORT` (default `4014`) and registers itself with the connector-manager on startup via the `CONNECTOR_MANAGER_URL` env var.

### Running tests

(No tests currently — PR welcome.)

### Rebuilding the Docker image

```bash
docker build -f connectors/telegram/Dockerfile -t omni-telegram-connector:latest .
```

(Build context must be the repo root because the Dockerfile copies `connectors/telegram/` as a relative path.)
