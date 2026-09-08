# Salesforce MCP Integration Plan

## Status

Initial implementation is enabled by default with
`SALESFORCE_MCP_ENABLED=true`.
Per-user OAuth and JWT authentication, disposable per-process CLI state
isolation, pinned runtime packages, tool-policy configuration, optional
resources/prompts handling, native-action preservation, and credential-ready
re-registration are implemented. Live Salesforce sandbox validation remains
required before production rollout.

## Decision

Keep the existing Salesforce native actions and add Salesforce's official DX MCP
server alongside them.

The official package is `@salesforce/mcp` (`sf-mcp-server`). It is a stdio MCP
server, not an HTTP server. It expects Salesforce CLI-authorized orgs and is
configured with an org alias plus selected toolsets/tools.

The Omni connector already has generic stdio MCP support. The implementation
will use that support rather than introducing a sidecar or an HTTP bridge.

## Current capabilities and constraints

### Existing native Salesforce actions to preserve

- `find_records`
- `get_case`
- `create_case`
- `update_case_status`
- `create_task`
- `update_task_status`

These remain the source of CRM-specific record actions because the official MCP
server does not currently provide equivalent generic Case/Task CRUD tools.

### Official MCP tools relevant to this integration

The official server currently includes, among others:

- Core: `get_username`, `resume_tool_operation`
- Data: `run_soql_query`
- Orgs: `list_all_orgs`, `create_scratch_org`, `delete_org`,
  `create_org_snapshot`, `open_org`
- Metadata: `deploy_metadata`, `retrieve_metadata`
- Users: `assign_permission_set`
- Testing: `run_agent_test`, `run_apex_test`
- DevOps Center and developer-focused LWC/code-analysis tools

The official Data toolset is currently primarily query-oriented. It does not
replace the connector's CRM actions.

### Existing Omni MCP behavior

The Python SDK already:

- supports `StdioMcpServer`;
- discovers MCP tools, resources, and prompts;
- converts MCP tools into connector manifest actions;
- dispatches MCP actions before falling back to native connector actions;
- bootstraps and caches an MCP catalog during sync.

The integration must account for the fact that manifest registration does not
have source credentials at process startup. The first credentialed discovery
must be made visible to connector-manager after bootstrap.

## Target architecture

```text
Omni action request
        |
        v
Salesforce connector HTTP server
        |
        +-- native action dispatch (unchanged)
        |
        +-- Omni MCP adapter
                |
                v
        /usr/local/bin/omni-salesforce-mcp wrapper
                |
                +-- ensure Salesforce CLI auth for this credential set
                |
                +-- exec sf-mcp-server --orgs <alias> ...
```

The wrapper is important because the official server expects a locally
authorized Salesforce CLI org, while Omni stores encrypted JWT or bearer
credentials. The wrapper translates the Omni credential into an isolated CLI
auth context before starting `sf-mcp-server`.

## Implementation phases

### Phase 1: Pin and package the runtime

Update `connectors/salesforce/Dockerfile` to install pinned runtime tools:

- Node.js 22;
- Salesforce CLI (`@salesforce/cli@2.150.6`);
- `@salesforce/mcp@0.30.15`;
- the MCP wrapper script.

Do not rely on runtime `npx` downloads. Installing the exact package in the
image gives reproducible builds and avoids network access during every MCP
request.

Add image smoke checks for:

```text
node --version
sf --version
sf-mcp-server --help
```

The image must continue to run the existing Python connector entrypoint.

### Phase 2: Add the Salesforce MCP launcher

Add a launcher, for example:

```text
connectors/salesforce/bin/omni-salesforce-mcp
```

Responsibilities:

1. Read the credential bridge values supplied by the connector.
2. Derive a stable, non-secret alias from a SHA-256 credential fingerprint.
3. Create an isolated `HOME` (and related Salesforce CLI state) with mode
   `0700` for that credential set. This prevents one Salesforce source from
   selecting another source's authorized org.
4. Acquire a per-source lock so concurrent MCP requests cannot corrupt the
   CLI auth store.
5. Run `sf org login jwt` with a securely-created temporary key file. The key
   is removed when the MCP process exits. Bearer-token MCP login is deferred
   because the pinned CLI requires the compound `org-id!token` format.
6. Start the official server with:

```text
sf-mcp-server \
  --orgs <derived-alias> \
  --toolsets <configured-toolsets> \
  --tools <configured-tools> \
  --no-telemetry
```

7. Start `sf-mcp-server` with stdin/stdout reserved for MCP and diagnostics
   on stderr.
8. Send diagnostics only to stderr. Never write credentials or private keys to
   stdout, MCP responses, Docker logs, command-line arguments, or persistent
   application logs.
9. Remove temporary JWT key files and stale auth directories according to a
   bounded cleanup policy.

The exact Salesforce CLI login syntax must be validated in a sandbox before
implementation is considered complete. The wrapper should fail explicitly with
a typed/logged reason for unsupported or malformed credentials.

### Phase 3: Bridge Omni credentials to the launcher

Update `SalesforceConnector` to override `prepare_mcp_env()` and supply only
the values the launcher needs.

JWT credentials:

- client ID;
- username;
- private key;
- login/instance URL;
- auth mode.

Bearer credentials are still accepted by native Salesforce actions but are
not passed to MCP in this first rollout; `prepare_mcp_env()` rejects them
explicitly until the pinned CLI access-token flow is validated end to end.

Do not pass the Omni database credential envelope or unrelated source fields.
Validate the mapping before launching the process.

The launcher environment should also include the configured MCP tool policy,
for example:

- `SALESFORCE_MCP_TOOLSETS`;
- `SALESFORCE_MCP_TOOLS`;
- `SALESFORCE_MCP_ALLOW_NON_GA_TOOLS`;
- `SALESFORCE_MCP_NO_TELEMETRY`.

### Phase 4: Enable MCP in the connector

Add an `mcp_server` property to `SalesforceConnector` returning a
`StdioMcpServer` pointing at the launcher.

Use a fixed launcher command and configuration-driven tool policy. Do not put
credential values in the static `mcp_server` definition.

Keep the existing native `actions` property and `execute_action()` unchanged.
The SDK's MCP-first dispatch will handle official MCP tool names, and native
actions will remain the fallback for the existing Salesforce-specific names.

Explicitly verify that there are no name collisions. If a future MCP tool name
matches a native action, native names should remain reserved and the collision
should be logged and excluded from the MCP catalog.

### Phase 5: Make credentialed catalog discovery reliable

The current SDK bootstraps MCP during the first sync, but the initial manifest
may be registered before credentials exist. The current implementation keeps
that first-sync path as a recovery path; bootstrap failures are isolated from
native sync. The next step is a generic org-credential-ready notification
parallel to the existing OAuth credential-ready flow, followed by manifest
re-registration so MCP actions become visible without restarting the
connector.

Catalog behavior:

- cache the MCP catalog only after successful authenticated discovery;
- include the configured tool policy and MCP package version in the cache key;
- invalidate the cache when credentials, org alias, package version, or tool
  policy changes;
- do not expose a stale catalog as authenticated if the auth context is gone;
- continue allowing sync and native actions if MCP discovery fails.

The resulting manifest should contain both native Salesforce actions and
MCP-discovered actions, with `mcp_enabled=true` and
`mcp_catalog_loaded=true` after successful bootstrap.

### Phase 6: Configuration and safety policy

Start with an explicit allowlist rather than enabling every official tool.
Suggested defaults for the first rollout:

```text
SALESFORCE_MCP_ENABLED=true
SALESFORCE_MCP_TOOLSETS=data
SALESFORCE_MCP_TOOLS=
SALESFORCE_MCP_ALLOW_NON_GA_TOOLS=false
SALESFORCE_MCP_NO_TELEMETRY=true
```

`data` exposes the initial read-oriented MCP functionality. Operators can opt
into additional toolsets, such as `metadata`, `users`, `testing`, or `orgs`,
after reviewing their write capabilities.

If per-source policy is needed later, add validated source-config fields for
MCP toolsets and tools. Do not allow arbitrary command-line arguments from the
web UI.

Document that these toolsets can perform privileged operations:

- metadata deployment can modify an org;
- user tools can change permissions;
- org tools can create/delete org resources;
- DevOps tools can commit or promote changes.

The connector should log the selected tool names/toolsets, but never log
credentials.

### Phase 7: Tests

#### Unit tests

Add tests for:

- JWT credential validation and launcher environment construction;
- bearer credential validation and launcher environment construction;
- deterministic alias generation without exposing secrets;
- sandbox/login URL selection;
- temporary key-file permissions and cleanup;
- concurrent login locking;
- stale/expired CLI auth recovery;
- unsupported credentials and malformed private keys;
- MCP tool-policy parsing and validation;
- native/MCP action-name collision handling.

#### Protocol tests

Run a fake stdio MCP server through the Omni adapter to verify:

- tools are discovered and mapped into `ActionDefinition` objects;
- read-only annotations map to `mode="read"`;
- tool calls return successful and error `ActionResponse` values;
- resources/prompts do not break manifest generation;
- native actions remain available when MCP is unavailable.

#### Docker tests

Build the connector image and verify:

- `sf`, Node, and `sf-mcp-server` are present;
- the wrapper preserves MCP stdout framing;
- stderr diagnostics do not contaminate the protocol;
- the connector starts without Salesforce credentials;
- the connector health endpoint remains healthy when MCP is disabled.

#### Sandbox acceptance tests

With a dedicated Salesforce sandbox:

1. Authenticate through JWT and through bearer mode separately.
2. Start the connector with `data` enabled.
3. Verify `run_soql_query` appears in the connector manifest.
4. Invoke `run_soql_query` through Omni.
5. Invoke each existing native action and confirm behavior is unchanged.
6. Enable `metadata`, `users`, and `testing` one at a time and verify the
   expected tools appear.
7. Restart the connector and confirm the catalog/auth cache behavior.
8. Rotate or revoke credentials and confirm old auth state is not reused.
9. Confirm failed MCP authentication does not prevent Salesforce sync from
   reporting its own explicit result.

### Phase 8: Documentation and rollout

Document:

- required Salesforce CLI/MCP package versions;
- supported JWT and bearer authentication modes;
- how the connector creates and isolates CLI org aliases;
- supported toolsets and their risk levels;
- how to enable/disable MCP;
- how to troubleshoot `sf org login` and MCP discovery;
- the distinction between native Salesforce actions and MCP tools.

Roll out with `SALESFORCE_MCP_ENABLED=true` and the read-only `data`
toolset:

1. Merge the image/runtime and launcher changes.
2. Run the sandbox acceptance suite.
3. Inspect connector-manager manifests and action dispatch logs.
4. Add privileged toolsets only after explicit operator configuration.

## Resolved rollout decisions

1. The first rollout requires JWT for MCP; bearer support follows after the
   pinned CLI access-token flow is validated.
2. Tool policy is global environment configuration for the first rollout.
3. Credential-ready re-registration should be implemented generically after
   the first-sync path is validated.
4. The catalog TTL remains the SDK default (24 hours), with atomic writes and
   policy-aware cache keys.
5. Only the `data` toolset is approved initially; privileged toolsets require
   explicit operator review.

## Definition of done

- The Salesforce image contains pinned Node, Salesforce CLI, and official MCP
  packages.
- The connector launches `sf-mcp-server` through stdio without a sidecar.
- JWT credentials can be translated into an isolated Salesforce CLI auth
  context programmatically.
- MCP tools are discovered, cached, shown in the connector manifest, and
  dispatched through Omni.
- Existing native Salesforce actions remain present and fully functional.
- MCP failures do not break Salesforce sync or native actions.
- Secrets are not exposed in command arguments, logs, MCP stdout, or shared
  auth directories.
- Sandbox acceptance tests cover authentication, query execution, catalog
  refresh, restart behavior, and native-action regression coverage.
