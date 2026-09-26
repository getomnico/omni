# Salesforce connector

Salesforce supports scheduled Salesforce REST API sync plus native actions. The
`run_soql_query` and `get_username` actions use the acting user's Salesforce
OAuth credential for both sync-enabled and no-sync sources; they never use the
source's sync credential. Their API identity must match the source's stored
Salesforce organization binding. Salesforce enforces the token holder's CRUD,
FLS, and row-level permissions.

`run_soql_query` retains the official `@salesforce/mcp@0.30.15` input names
(`query`, `usernameOrAlias`, `directory`, and optional `useToolingApi`),
`content` result field, and text-result prefix. `directory` is accepted for compatibility but is never
used to access the filesystem. Queries are restricted to one read-only SELECT,
limited to 100 pages, 10,000 records, 8 MB output, and 90 seconds. Within those
bounds, large JSON action results flow through the AI service's existing
`text_result_or_sandbox` handling, which stores large results in the chat's
sandbox workspace when available; the connector itself never writes files or
accepts a caller-controlled path. Results beyond the hard bounds fail with an
instruction to narrow the query. Tooling API queries use the caller's same OAuth
token.

## Official MCP catalog treatment

The prior connector enabled the `data` toolset and the always-on `core`
toolset in `@salesforce/mcp@0.30.15` (whose pinned `dx-core` provider is
0.9.8). The resulting exposed catalog was:

- `run_soql_query` and `get_username`: retained as native, user-OAuth actions.
- `resume_tool_operation`: deliberately dropped. It resumes deploy,
  scratch-org, and agent-test operations, not SOQL queries; there is no native
  replacement.
- `list_all_orgs`: deliberately not exposed. It lists a local Salesforce CLI
  allowlist, not orgs available via this source's actor token. A native
  equivalent would be misleading and risk disclosing unrelated local CLI orgs.

No other MCP toolsets were enabled. Salesforce source OAuth remains declared
in the connector manifest. The Salesforce MCP process, launcher, CLI bootstrap,
Node image stage, and Salesforce-specific MCP dependency have been removed;
the generic SDK MCP support for other connectors is unchanged.
