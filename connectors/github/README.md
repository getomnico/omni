# GitHub connector

Syncs repositories (README and metadata), issues, pull requests (with comments) and
Discussions from github.com or GitHub Enterprise Server (GHES) into Omni. AI actions
run through `github-mcp-server`.

## Setup

1. Create a personal access token with the `repo` and `read:org` scopes.
2. In Omni, go to Settings → Integrations → GitHub → Connect and paste the token.
3. Optionally set an API URL (GHES only; leave blank for github.com).

## Source configuration

| Field                | Description                                                             |
| -------------------- | ----------------------------------------------------------------------- |
| `api_url`            | GHES API URL. Blank or `https://api.github.com` means github.com.       |
| `api_version`        | Optional REST API version (`YYYY-MM-DD`) override. Only used with `api_url`. |
| `include_discussions`| Index Discussions (default `true`).                                     |
| `include_forks`      | Include forked repositories (default `false`).                          |
| `repos` / `orgs` / `users` | Restrict the sync to these `owner/name` repos, organizations or users. |
| `read_only`          | Disable write actions (default `false`).                                |

## GitHub Enterprise Server

- **API URL**: `https://<host>`, `https://<host>/` and `https://<host>/api/v3` are all
  accepted and stored as `https://<host>/api/v3`.
- **Token**: create it at `https://<host>/settings/tokens` with `repo` and `read:org`.
- **REST API version**: on github.com the connector uses githubkit's latest version.
  With an API URL it sends `2022-11-28`, the version every supported GHES release
  accepts. GHES 3.21+ can opt into `2026-03-10` via `api_version`.
- **Discussions** use GraphQL at `https://<host>/api/graphql` (GHES 3.6+). If the server
  rejects the query, the connector logs a warning and skips discussions.
- **GitHub Enterprise Cloud with data residency** (`*.ghe.com`): enter
  `https://api.<subdomain>.ghe.com`; it is kept as typed. AI actions are not supported
  for GHE.com yet.
- **AI actions** start `github-mcp-server` with `GITHUB_HOST=https://<host>`; the tool
  catalog is discovered during sync with the same host.
- **Limitation**: per-user OAuth (the OAuth Apps tab) targets github.com only, because the
  connector manifest cannot declare per-source OAuth endpoints. GHES sources use the PAT.

## Development

```bash
uv sync
uv run pytest tests/test_ghes.py   # unit tests, no Docker
uv run pytest                      # full suite, integration tests need Docker
```
