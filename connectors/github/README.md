# GitHub Connector

Connects GitHub repositories, issues, pull requests, and discussions into the
Omni index, and exposes GitHub tools to agents through the bundled
[github-mcp-server](https://github.com/github/github-mcp-server) (stdio).

## Authentication

Two credential models, each with a distinct job:

| Credential | Stored as | Used for | Setup |
| --- | --- | --- | --- |
| **Org credential** | `service_credentials` row with `user_id IS NULL` | Indexing/sync and org-level agents | Admin: "Connect with GitHub" (OAuth, `org_source` flow) or a PAT |
| **Per-user credential** | `service_credentials` row keyed by `user_id` | All user-invoked MCP actions | User connects their own account (OAuth, `user_write` flow) |

### Per-user OAuth (actions)

When a user invokes a GitHub MCP action (or loads an MCP-prompt-backed skill),
connector-manager resolves credentials for **that user**:

- Per-user credential exists → the user's own OAuth token is bridged to
  `github-mcp-server` via `GITHUB_PERSONAL_ACCESS_TOKEN`, so actions are
  attributed to and limited by the user's GitHub identity.
- No per-user credential → connector-manager returns `412 needs_user_auth`
  (an OAuth connector must never fall back to the org credential). The chat UI
  renders a "Connect GitHub" card that starts
  `/api/oauth/start?source_id=...&flow=user_write`. Users can also connect
  proactively from **Settings → Integrations**.

### OAuth scopes

Declared by the connector manifest (`oauth_config()`):

- Identity (all flows): `read:user`, `user:email`
- Read/sync flows (`org_source`, `user_read`): `repo`, `read:org`
- Write/action flows (`user_write`): `repo`

The `user_write` flow enforces exact scope coverage (strict check): the
granted token must cover every required scope or the callback redirects with
an error. Note that a GitHub **OAuth App** token with `repo` is all-or-nothing
across every repo the account can access; fine-grained per-repository grants
require a GitHub **App** (installation) flow — a known limitation, documented
here as follow-up work.

### Sync

Sync always uses the org credential (it is a service-level operation,
independent of who is chatting). Sync honors the source config
`repos` / `orgs` / `users` lists; when none are configured it falls back to
all repositories visible to the org credential's identity.

## MCP tool surface

The connector runs `github-mcp-server stdio` with a curated toolset list
(`MCP_TOOLSETS` in `github_connector/config.py`): `context`, `repos`, `git`,
`issues`, `labels`, `pull_requests`, `discussions`, `actions`, `users`.
Deliberately excluded: gists, notifications, orgs, projects, dependabot,
code/secret security, security advisories, stargazers, and copilot toolsets.

- Tools annotated read-only by the MCP server map to Omni `mode=read`;
  everything else maps to `mode=write` and requires interactive chat approval.
- If the source is marked read-only, connector-manager rejects write actions
  regardless of the tool surface.

### Repository scoping

MCP tool calls are validated against the source's configured scope before
dispatch (`validate_mcp_action`):

- `repos` configured → tool calls must target repositories in that list.
- `orgs` / `users` configured → tool calls must target owners in those lists
  (any repository under them is in scope, mirroring what sync indexes).
- No scope configured (discovery mode) → no constraint can be derived; calls
  are allowed and the limitation is inherent to the token's reach.
- Calls without `owner` (e.g. `get_me`) are always allowed.

Out-of-scope calls fail explicitly with a 400 error — they are never silently
rewritten.

## Development

```bash
uv run pytest tests/test_mcp.py   # MCP/OAuth unit tests (no infra)
uv run pytest                     # full suite (testcontainers-based)
```

The bundled `github-mcp-server` version is pinned in `Dockerfile`; toolsets
and tool names in the skills reference that version.
