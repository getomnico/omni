# Salesforce MCP Per-User OAuth Plan

## Objective

Make every Salesforce MCP tool invocation use the calling Omni user's
Salesforce OAuth credential. The organization JWT credential may continue to
serve synchronization and explicitly org-scoped native operations, but it must
never be used as an MCP fallback.

## Current gap

The original implementation launched `sf-mcp-server` with the source
owner's JWT credential, marked discovered MCP tools admin-only, and resolved
them from the org credential. This was not acceptable for user-scoped MCP
access. The implementation now fails closed on MCP requests while Salesforce
OAuth client configuration and sandbox validation are completed. Salesforce
DCR is not a secretless public-client flow: its registration endpoint requires
an authorized initial bearer credential and returns confidential-client
credentials.

## Decisions

- MCP actions require a resolved `user_id`.
- MCP actions require a per-user Salesforce OAuth credential; missing user
  credentials return the standard `NeedsUserAuth` response.
- MCP dispatch must fail closed: it must not fall back to the org JWT.
- Native Salesforce sync continues using the org JWT credential.
- Existing native Salesforce actions remain available and are not replaced by
  MCP tools.
- Use Salesforce External Client App OAuth authorization-code/PKCE for users.
  Keep the JWT service app separate where practical to simplify scopes,
  revocation, and auditing.
- Use an admin-created Salesforce External Client App for the initial
  user-facing OAuth client. Salesforce advertises dynamic registration through
  OIDC discovery, but its registration endpoint requires an authorized initial
  bearer credential and returns confidential-client credentials; it is a
  separately validated follow-up, not an unauthenticated public-client path.
- Bind OAuth configuration to the Salesforce source/login issuer and expected
  organization ID. Existing-source flows use a source-scoped OAuth client
  configuration key (with the provider-global key retained only for initial
  source creation), because provider-global client configuration is
  insufficient for multiple Salesforce orgs or sandbox domains.
- Carry explicit MCP/native action provenance through dispatch and reserve
  native action names. MCP authentication failures must never execute native
  actions.
- Apply the same user credential boundary to MCP resources, prompts, and
  MCP-backed skills.
- The initial MCP tool policy remains the `data` toolset. Metadata, users,
  orgs, testing, and DevOps toolsets remain disabled until separately reviewed.

## Target request flow

```text
Omni user invokes an MCP action
  -> connector-manager requires user_id
  -> resolve per-user Salesforce OAuth credential
  -> NeedsUserAuth if absent
  -> connector receives only that user's credential
  -> launcher creates state isolated by source_id + user_id
  -> sf org login access-token / supported user OAuth auth
  -> sf-mcp-server runs with that user's org identity
```

## Implementation phases

### 1. Add MCP action provenance and authorization policy

- Keep explicit MCP provenance in the manifest's `mcp_action_names` and carry
  `origin=native|mcp` on the connector action request; native action names are
  reserved and cannot be reclassified by live MCP discovery.
- Populate `source_types` for MCP actions as today, but do not use an empty or
  source-type heuristic to identify MCP actions.
- Make MCP tools user-scoped rather than `admin_only`.
- In connector-manager, require `user_id` for MCP actions and reject MCP
  requests without an actor before credential resolution.
- Extend credential resolution with a strict `require_user_credential` mode:
  no per-user row must never fall back to the org row.
- Keep the existing admin-only/org-credential path for native administrative
  actions.
- Apply the same user-credential requirement to MCP resource, prompt, and
  skill requests. Extend credential envelopes and skill request APIs with
  trusted source/user/provider identity; static skills may remain non-MCP, but
  MCP-backed skills must never load owner credentials implicitly.

### 2. Declare Salesforce per-user OAuth

- Implement `SalesforceConnector.oauth_config()` for Salesforce's External
  Client App authorization-code/PKCE flow.
- Discover the Salesforce login domain's OIDC metadata from
  `https://<login-domain>/.well-known/openid-configuration` and use its
  authorization, token, and userinfo endpoints. Registration metadata is
  not enabled until the separately authenticated DCR phase; do not enable it
  as an unauthenticated public-client flow. Do not hardcode production
  endpoints for sandbox sources.
- Configure the manifest for the admin-created app using the Salesforce
  authorization and token endpoints, with confidential-client credentials
  stored through the existing encrypted admin OAuth configuration.
- Use authorization-code + PKCE for the user flow, while keeping the token
  endpoint authentication method required by the configured app (normally
  `client_secret_post` or `client_secret_basic`). PKCE and client
  authentication are independent controls.
- Support Salesforce dynamic registration only when an administrator supplies
  the required initial bearer credential. Send the provider-specific
  payload/scopes, securely store the returned client secret, throttle repeated
  registration attempts, and retain the External Client App fallback. Do not
  treat it as a secretless public-client registration.
- Validate every discovered endpoint against the Salesforce-host allowlist and
  existing OAuth SSRF protections before using it.
- Use `api` for read access initially; request write scopes only where the
  user-facing action policy requires them.
- Configure the External Client App for user authorization, PKCE/public-client
  behavior as supported by the selected Salesforce flow, and pre-authorize or
  permit the intended users/policies.
- Ensure the OAuth callback is the Omni callback generated by the web OAuth
  flow, not the Salesforce JWT placeholder callback.
- Store the resulting per-user OAuth credential under the Salesforce source
  and user identity using the existing service-credential path.
- Validate OAuth userinfo `organization_id` and `instance_url` against the
  source's configured Salesforce organization before storing the credential.
- Preserve the org JWT credential separately for sync.

### 3. Separate source setup from user authorization

Update the Salesforce setup UI and documentation to make the distinction
explicit:

- Admin setup: org JWT credential for sync and native source health.
- User authorization: standard Omni OAuth flow for each user before MCP use.
- Do not ask users to paste access tokens into the source setup form.
- Keep the static access-token tab only for native-action trials, or remove it
  if it cannot be clearly separated from MCP authorization.
- Show a clear `Connect Salesforce for MCP` / `Authorize Salesforce` action
  when connector-manager returns `NeedsUserAuth`.
- Do not expose MCP tools as callable by an org-level agent without a user.

### 4. Update the CLI/MCP launcher for per-user credentials

- Accept the resolved user OAuth credential, not the org JWT, for MCP action
  requests.
- Use an isolation key derived from `source_id` and `user_id`; never use only
  the source id.
- Validate the Salesforce instance/login URL before launching subprocesses.
- Validate token and organization identity fields without logging secrets.
- Use the pinned CLI's supported access-token login flow. If it requires
  `org-id!access-token`, persist the OAuth provider's organization ID with the
  credential or obtain it through the authenticated user-info endpoint before
  invoking the CLI.
- Do not put tokens in command-line arguments. Use the CLI-supported secure
  environment/input mechanism and keep temporary files mode `0600`.
- Keep per-user lock/state directories mode `0700`, clean up temporary secrets,
  and bound stale-lock recovery and subprocess lifetime.
- Pass only the `data` toolset and the configured allow-list to
  `sf-mcp-server`.
- Ensure MCP stdout remains a clean protocol channel; diagnostics go to stderr.

If the pinned CLI cannot safely consume the provider's user OAuth token, stop
MCP dispatch with a clear unsupported-auth error rather than using the org JWT.

### 5. Make catalog and manifest handling user-safe

- Tool schemas may be cached globally because they are not credentials, but
  the cache key must include the MCP package version and tool policy.
- Never treat a cached catalog as proof that a user is authorized.
- Authenticate every MCP action/resource/prompt session with the resolved
  user's credential.
- Keep credentialed discovery optional; discovery failures must not break sync
  or native actions.
- Re-register the manifest after connector changes, but do not advertise a
  tool as user-authorized merely because it appears in the manifest.
- Log action origin and credential class (`user` vs `org`) without logging
  tokens, private keys, or authorization codes.

### 6. Tests and acceptance criteria

#### Authorization tests

- MCP action without `user_id` is rejected.
- MCP action with no user credential returns `NeedsUserAuth`.
- MCP action never resolves or dispatches with the org JWT.
- User credentials for user A cannot select user B's CLI state.
- Native sync still uses the org JWT.
- Native Salesforce actions retain existing behavior.
- MCP resources, prompts, and skills enforce the same user boundary.
- Admin-only native actions still use the org credential where intended.

#### OAuth tests

- OIDC discovery for production and sandbox login domains, with strict
  issuer/host/origin validation and no unsafe redirects.
- Admin-created External Client App configuration, PKCE, confidential client
  authentication, callback, grants, and scopes.
- Optional authenticated DCR request/response, including the initial bearer
  credential, returned client secret, registration limits, and cleanup.
- External Client App authorization and callback registration.
- Production and sandbox login URLs.
- Token refresh and expired-token recovery.
- Revoked authorization requires re-authentication; token refresh/login
  failures map to `NeedsUserAuth` and invalidate the unusable user credential
  instead of returning a generic connector error.
- OAuth scopes map correctly to read/write tool policy.

#### Launcher/protocol tests

- User-specific state and lock paths are deterministic and non-secret.
- Access-token login works with the pinned Salesforce CLI, including org ID
  handling.
- Temporary secret files are private and removed.
- Concurrent requests cannot corrupt one user's CLI state.
- A failed MCP login does not break native actions or sync.
- MCP stdout framing remains valid and optional resources/prompts do not break
  discovery.

#### Sandbox acceptance

1. Configure an org JWT External Client App for sync.
2. Configure an admin-created Salesforce External Client App for per-user
   OAuth/PKCE and verify its callback, scopes, and confidential-client
   authentication.
3. Optionally provide the Salesforce DCR initial access token and verify the
   returned client secret, registration throttling, and cleanup; otherwise
   retain the External Client App configuration.
4. Authorize two Salesforce users through Omni.
5. Invoke `run_soql_query` as each user and verify Salesforce identity and
   permissions differ correctly.
6. Invoke without authorization and verify the OAuth prompt path.
7. Revoke one user's authorization and verify only that user loses MCP access.
8. Confirm native sync and native Salesforce actions still work.
9. Confirm no MCP request succeeds using only the org JWT.

## Rollout

1. Implement strict fail-closed authorization before enabling user MCP tools.
2. Deploy the OAuth manifest and UI flow with MCP tools unavailable until a
   user credential exists.
3. Validate the pinned CLI user-token flow in a Salesforce sandbox.
4. Enable the `data` toolset for a small test group.
5. Audit connector-manager logs and Salesforce login history.
6. Expand access only after identity, scope, revocation, and native-action
   regression tests pass.

## Definition of done

- Every MCP invocation is tied to an Omni user and a Salesforce OAuth identity.
- The org JWT is never used for MCP fallback.
- Missing/revoked user authorization produces an OAuth prompt or a clear
  authorization error.
- Native sync and native Salesforce actions remain functional.
- Per-user CLI state, tokens, locks, and subprocesses are isolated and cleaned
  up safely.
- Sandbox acceptance tests verify identity separation and revocation.
