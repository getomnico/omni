# Snowflake connector

Omni's Snowflake connector uses Snowflake's first-party managed Streamable HTTP MCP
server and an official `snowflake-connector-python` client.

## Integration shape

This is a hybrid metadata sync plus provider-managed MCP integration. The optional
metadata sync indexes databases, schemas, tables/views, columns, comments, selected
structure, tags, and conservative role permissions. It never indexes warehouse rows,
query results, samples, or query history. MCP tools remain live and Snowflake enforces
MCP-server, object, masking, row-access, and effective-role permissions for the
invoking user.

The connector supports multiple Snowflake sources and publishes one union manifest.
Endpoints are source configuration, not connector-wide state. Conflicting tool names
are omitted rather than exposed ambiguously. MCP write tools are disabled by default
at the product layer and every user-facing call requires that user's Snowflake OAuth
credential; the metadata service credential is never a fallback.

## Setup

1. Create a least-privileged metadata role. Snowflake's documented
   `SNOWFLAKE.OBJECT_VIEWER` and `SNOWFLAKE.SECURITY_VIEWER` database roles are the
   starting point; grant `USAGE` on a small warehouse and database/schema metadata
   visibility. Do not grant application table-row access solely for Omni indexing.
2. Configure an account URL, metadata warehouse/role, database allowlist, and key-pair
   service user in the Omni integration settings. Rotate the key by replacing the
   service credential; passwords are not supported.
3. For MCP, create a managed server at
   `https://<account>/api/v2/databases/<database>/schemas/<schema>/mcp-servers/<name>`.
   Grant `USAGE` on that server and explicit privileges for its tools. Prefer a
   least-privileged role, `OAUTH_USE_SECONDARY_ROLES = NONE`, a restricted
   `ALLOWED_ROLES_LIST`, and governed Cortex objects over unrestricted SQL.
4. Snowflake managed MCP does not support Dynamic Client Registration. Create a
   confidential OAuth security integration, enter its source-scoped client ID/secret,
   and use Omni's exact callback URL. Omni stores that client under the Snowflake
   source ID rather than sharing it across accounts. The administrator must then click **Connect
   Snowflake and discover tools**. Discovery uses that consenting administrator only
   for `tools/list`; execution always resolves the invoking user's credential.

Account Usage has provider latency (including multi-hour visibility delays). Omni uses
an overlap window, conservative cutoff, idempotent upserts, and periodic full repair.
The connector advertises only `full` and `incremental`; it does not claim realtime
metadata synchronization.

## Troubleshooting

- Verify the account URL and managed MCP path are on the same Snowflake account and
  use HTTPS without query strings, fragments, credentials, or unexpected ports.
- Check the metadata role, warehouse `USAGE`, Account Usage roles, network policy, and
  the database/schema allowlists.
- A missing or expired user OAuth credential returns `needs_user_auth`; reconnect that
  user rather than expecting the service user to work for MCP.
- A stale catalog can be refreshed from the integration settings. Restart recovery
  retains the Redis catalog while authenticated discovery is pending; if Redis is
  lost, a stored valid per-user discovery credential is replayed.

The local icon is a compact derivative mark stored at
`web/src/lib/images/icons/snowflake.svg`; use Snowflake's official brand guidance for
production branding. The integration references Snowflake's public documentation:
<https://docs.snowflake.com/en/user-guide/snowflake-cortex/cortex-agents-mcp>.
