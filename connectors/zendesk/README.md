# Zendesk connector

Read-only sync of Zendesk Support **tickets** and Help Center **articles** into Omni. Sync modes: `full` and `incremental` (no realtime).

## What is indexed

- **Tickets**: subject, description, every comment (including internal notes, marked `[internal note]`), requester, assignee, group, organization, tags, and attachment *file names*. Closed and archived tickets are included; soft-deleted tickets are removed from the index.
- **Help Center articles**: published articles only (HTML body). Drafts are skipped; an article that was indexed and goes back to draft is removed on the next sync.

Search operators: `status`, `priority`, `type`, `tag`, `assignee`, `requester`.

## Permissions

Documents fail closed. Omni groups are created per source:

- `zendesk:{source_id}:agents`: active, non-suspended admins and agents without a ticket restriction.
- `zendesk:{source_id}:group:{id}`: active agents restricted to `groups` who belong to that Zendesk group.

Tickets are visible to the agents group plus the group of the ticket's Zendesk group. Articles are public only when neither `user_segment_id` nor `user_segment_ids` is set; otherwise they are visible to the agents group.

Limits in v1:

- Agents restricted to `assigned`, `requested` or `organization` tickets see no tickets.
- End users are never granted anything.
- Members are matched by e-mail address (lower-cased); users without an e-mail are skipped.

## Setup

Choose one credential kind. Both must belong to an **admin**: the incremental ticket export is admin-only.

**API token (recommended)**

1. In Zendesk go to **Admin Center → Apps and integrations → APIs → Zendesk API**, enable **Token access** and **Add API token**.
2. In Omni add a Zendesk source with the subdomain (`acme` for `https://acme.zendesk.com`), the admin e-mail and the token.

**OAuth access token (advanced)**

Any OAuth access token for the account with read access, belonging to an admin. In Omni add the source with the subdomain and the access token. The connector cannot refresh it: when it expires, syncs fail with HTTP 401 until a new token is saved on the source's settings page.

The credential check calls `GET /api/v2/users/me.json` and rejects credentials that do not belong to an admin.

Source config: `{ "subdomain": "acme", "sync_tickets": true, "sync_help_center": true }`. The subdomain cannot be changed after creation.

To rotate the token, create a new one in Zendesk and update the source credentials in Omni, then revoke the old token.

## Rate limits and first-sync duration

The client sends about 5 requests/second and retries 429 and 5xx responses, honouring `Retry-After` (capped at 60 s, default 10 s). The ticket export family allows 10 requests/minute and returns up to 1000 tickets per page, so it is rarely the bottleneck. Comments are fetched with **one request per ticket** (per 100 comments), so the first full sync takes roughly `tickets / 5` seconds, e.g. about 3 hours for 50,000 tickets. Later incremental syncs only fetch changed tickets. Zendesk's per-plan API limits (for example 200 requests/minute on Team plans) can be below the connector's 5 requests/second, so first syncs may see 429 responses; they are retried.

## Sync behaviour

Phases: users, group memberships, articles, tickets. Users and memberships are re-read on every run. The checkpoint stores the ticket export cursor after each fully emitted page, so an interrupted run resumes from the last committed page. A full sync restarts the export from the beginning and reconciles deleted articles; incremental syncs continue from the stored cursor and walk articles newest-first until the previous watermark (with a 5 minute overlap). If a ticket's comments cannot be read because Zendesk answers 404, 410 or 422, the ticket is indexed with its description only; any other error (including 401/403) fails the run. Tickets purged from Zendesk without a soft-delete tombstone are not reconciled. Groups that disappear from Zendesk are emptied on the next run. If Zendesk rejects the stored export cursor (4xx other than 401/403), the run logs a warning and automatically restarts the export from the beginning.

## Not in v1

Attachment contents, OAuth token refresh, write actions, MCP tools, realtime sync, agents restricted to anything other than groups, Help Center sections/categories as paths, person sync, and removal of already indexed tickets when `sync_tickets` is turned off (full sync does remove articles when `sync_help_center` is turned off).

## Troubleshooting

- **Credential check says not authenticated**: wrong e-mail/token pair, token access disabled, or the user is not an admin.
- **HTTP 403 on `/incremental/tickets/cursor.json`**: the token does not belong to an admin.
- **Sync fails with "repeated a cursor"**: Zendesk returned the same pagination cursor twice; retry, and report it if it persists.
- **Sync fails with HTTP 401 when using an OAuth access token**: the token expired or was revoked; save a new token on the source's settings page (the connector cannot refresh it).
