# Windshift Connector

Indexes Windshift work items, descriptions, and comments into Omni and exposes
Windshift's MCP tools as Omni actions.

## Configuration

Set `WINDSHIFT_BASE_URL` to the externally reachable Windshift base URL. If
Windshift uses a context path, include it in the URL. Enable Windshift's MCP
server with `MCP_ENABLED=true`.

For local or private networking, `WINDSHIFT_INTERNAL_BASE_URL` may point the
connector container at the same Windshift instance through a different route.
Browser authorization, resource binding, and document links still use
`WINDSHIFT_BASE_URL`; server-side client registration, token exchange, user-info,
sync, and MCP requests use the internal route.

No OAuth client ID or secret is configured manually. Omni dynamically registers
as a public client, uses S256 PKCE, and requests tokens bound to
`${WINDSHIFT_BASE_URL}/mcp`. Windshift 0.8.3 or newer is required.

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

Full sync walks visible workspaces and items. Incremental sync stops once items
are older than the last checkpoint. Optional `workspace_keys` restricts sync to
specific Windshift workspaces. Windshift is a personal source in Omni: every
user has an independent sync backed by their own OAuth credential, and indexed
items are visible only to that Omni user.
