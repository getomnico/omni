# ClickUp Connector

The ClickUp connector keeps its existing REST-based sync implementation for indexing tasks and docs. Sync credentials still use the current ClickUp token flow and are not changed by the MCP integration.

## Remote MCP for actions

ClickUp actions use ClickUp's hosted Streamable HTTP MCP server at:

```text
https://mcp.clickup.com/mcp
```

No local ClickUp MCP server is required. Using the hosted endpoint avoids packaging or spawning a local MCP subprocess and follows ClickUp's official MCP tool surface.

## Authentication

The remote MCP endpoint requires OAuth bearer tokens. The connector forwards per-user OAuth access tokens as:

```http
Authorization: Bearer <access_token>
```

Legacy ClickUp personal/API tokens remain sync-only unless ClickUp documents them as valid MCP bearer tokens.

When a user invokes a ClickUp MCP action and has no per-user credentials, Omni should prompt the user through the existing `user_write` OAuth flow. ClickUp's MCP OAuth metadata advertises public-client OAuth with PKCE and dynamic client registration, so admins should not need to manually create a ClickUp OAuth app by default. A manually configured OAuth client can remain a fallback if dynamic registration is unavailable.

## Tool catalog discovery

ClickUp does not allow unauthenticated MCP tool/resource/prompt listing. The connector therefore only discovers tools after authenticated OAuth bootstrap and caches the resulting catalog for future manifest reads. Manifest generation itself must not make unauthenticated calls to `https://mcp.clickup.com/mcp`.
