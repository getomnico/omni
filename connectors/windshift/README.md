# Windshift Connector

Indexes Windshift work items, descriptions, and comments into Omni and exposes
Windshift's MCP tools as Omni actions.

## Configuration

Windshift is configured in the admin UI under **Settings > Integrations >
Windshift server**: enter the externally reachable Windshift base URL. No env
vars are required — the setting is stored in the `connector_configs` table and
the connector reads it from connector-manager, so changes propagate without a
container restart. Enable Windshift's MCP server with `MCP_ENABLED=true`.

Deployments that predate the UI setting can still set `WINDSHIFT_BASE_URL` as
a fallback; the UI setting wins when both are present. If Windshift uses a
context path, include it in the URL.

The saved URL is validated against SSRF (same policy as remote MCP sources):
only http(s), no credentials/fragments, and the resolved address must be
publicly routable.

### Internal route (env-only)

To keep server-to-server traffic (client registration, token exchange,
user-info, sync, MCP) off the public network, set `WINDSHIFT_INTERNAL_BASE_URL`
on the connector container to a private route to the same Windshift instance,
e.g. `http://windshift:8080` on the compose network. This is intentionally
env-only — it is not configurable in the UI. When set, the connector advertises
it in its manifest and Omni's OAuth flow allows that exact origin (scheme + host
+ port) to resolve to private (RFC1918) addresses; loopback, link-local/metadata,
and reserved ranges are still rejected, and every other endpoint must stay
publicly routable. Browser authorization, resource binding, and document links
always use the public URL regardless.

No OAuth client ID or secret is configured manually. Omni dynamically registers
as a public client, uses S256 PKCE, and requests tokens bound to
`${WINDSHIFT_BASE_URL}/mcp`. Windshift 0.8.4 or newer is required.

Each user connects Windshift from **My Integrations**. The initial authorization
grants read access for that user's sync and read-only MCP tools. Write and
destructive tools request expanded authorization when first used. Access tokens
are refreshed automatically; rotated refresh tokens are persisted under the same
per-credential database lock.

## Data model

| Windshift                  | Omni document                              |
| -------------------------- | ------------------------------------------ |
| Item ID                    | `external_id = windshift:item:<id>`        |
| Title                      | Document title                             |
| Description and comments   | Markdown content                           |
| Workspace                  | `attributes.workspace`                     |
| Status                     | `attributes.status`                        |
| Priority                   | `attributes.priority`                      |
| Assignee                   | `attributes.assignee` and `assignee_email` |
| Created/updated timestamps | Document metadata                          |

Full sync walks visible workspaces and items while capturing each workspace's
change watermark. Incremental sync then reads Windshift's ordered item change
log, including comment activity and deletions. Optional `workspace_keys`
restricts sync to specific Windshift workspaces. Windshift is a personal source
in Omni: every user has an independent sync backed by their own OAuth
credential, and indexed items are visible only to that Omni user.
