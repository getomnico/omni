# Salesforce connector setup

This guide records the setup used to connect Salesforce to Omni for both:

- **Native Salesforce sync and actions**, authenticated with an org-level JWT integration user.
- **Salesforce MCP tools**, authenticated with the currently signed-in Omni user's Salesforce OAuth credential.

These are deliberately separate authentication paths. MCP must not fall back to the org JWT.

## 1. Prerequisites

Before configuring Salesforce:

1. Deploy an Omni version containing the Salesforce connector and the official Salesforce MCP runtime.
2. Ensure the Omni web application is reachable at the URL that will be used as the OAuth callback.
3. Sign in to Omni as an administrator.
4. Sign in to Salesforce as an administrator who can create users and External Client Apps.
5. Have a secure location for the RSA private key. Do not commit it or paste it into screenshots, tickets, or chat.

For local development, the callback used in this setup was:

```text
http://localhost:3000/api/oauth/callback
```

For a shared or production deployment, use the deployment's reachable HTTPS callback instead. A temporary ngrok callback was used during part of the local testing, but the final local app configuration used `localhost`.

## 2. Create a dedicated Salesforce integration user

The native connector uses a dedicated Salesforce user so that synchronization does not depend on an individual employee's password or OAuth session.

In Salesforce:

1. Open **Setup → Users → Users**.
2. Select **New User**.
3. Create a dedicated user, for example `Omni Salesforce Integration`.
4. Use the **Salesforce Integration** user license when available.
5. Assign an API-only integration profile, such as **Minimum Access - API Only Integrations** or the Salesforce API-only integration profile available in the org.
6. Set a unique Salesforce username and email address.
7. Keep the user active.
8. Grant the object permissions required by the records that Omni should index. The connector syncs accounts, contacts, opportunities, leads, cases, and tasks.

The API-only profile in the test org did not expose the standard `Account` object to the native connection test. Verify object visibility and read permissions before considering synchronization complete.

## 3. Generate the JWT key pair

Generate an RSA private key and a matching self-signed public certificate outside the repository. For example:

```bash
umask 077
openssl genrsa -out privatekey.pem 2048
openssl req -new -x509 \
  -key privatekey.pem \
  -out server.crt \
  -days 3650 \
  -subj "/CN=Omni Salesforce Integration"
chmod 600 privatekey.pem
```

Keep `privatekey.pem` secret. Only the public certificate (`server.crt`) is uploaded to Salesforce.

## 4. Create the Salesforce External Client App

In Salesforce Setup:

1. Open **Apps → External Client Apps → External Client App Manager**.
2. Select **New External Client App**.
3. Set the app name to `Omni`.
4. Set the contact email to the administrator's email address.
5. Keep the distribution state **Local** for an org-local app.
6. Enable OAuth.
7. Set the callback URL to the Omni callback from [Prerequisites](#1-prerequisites).
8. Add the OAuth scopes required for the integration:
   - **Access the identity URL service** (`id`, `profile`, `email`, `address`, `phone`)
   - **Manage user data via APIs** (`api`)
   - **Manage user data via Web browsers** (`web`)
   - **Perform requests at any time** (`refresh_token`, `offline_access`)
   - **Access unique user identifiers** (`openid`)
   - **Access Salesforce hosted MCP servers** (`mcp_api`)

The selected scopes are shown in [External Client App settings](./salesforce-external-client-app-settings.png).

### Enable the required flows and security controls

In the External Client App's OAuth settings:

- Enable **Authorization Code and Credentials Flow** for per-user OAuth.
- Enable **JWT Bearer Flow** for the native connector's org authentication.
- Upload the public certificate generated in step 3 for the JWT bearer flow.
- Enable **Require Proof Key for Code Exchange (PKCE)** for the authorization-code flow.
- Keep refresh-token rotation and Salesforce's enforced idle refresh-token limits enabled.
- Keep **Issue JSON Web Token (JWT)-based access tokens for named users** enabled where required by the org configuration.

The flow and certificate settings are shown in [External Client App settings](./salesforce-external-client-app-settings.png).

### Configure authorization policy

In the External Client App's **Policies** tab:

1. Configure the permitted-user policy. The test setup used **Admin approved users are pre-authorized**.
2. Add the profiles that should be allowed to authorize the app. The test setup included the API-only integration profile and **System Administrator**.
3. Leave the OAuth Start URL empty unless the organization has a specific start page.
4. Save the app.

The resulting policy is shown in [External Client App policies](./salesforce-external-client-app-policies.png).

> If the org instead uses **All users can self-authorize**, keep that policy only if it matches the organization's security requirements. With admin pre-authorization, every intended user must be covered by the selected profile or permission assignment.

### Retrieve the client credentials

After saving the app, open **Consumer Key and Secret** and copy the values into a secure password manager or deployment secret store. Do not commit them or include them in screenshots.

The consumer key/secret page was intentionally not captured for this guide.

## 5. Connect the Salesforce source in Omni

In Omni:

1. Open **Admin → Settings → Integrations**.
2. Select **Connect** for Salesforce.
3. Choose **External Client App (JWT)**.
4. Enter:
   - **Consumer Key**: the Salesforce External Client App client ID.
   - **Private Key (PEM)**: the RSA private key from step 3.
   - **Username**: the dedicated Salesforce integration user.
   - **Login URL**: `https://login.salesforce.com` for production, or `https://test.salesforce.com` for a sandbox.
   - **Instance URL**: the exact Salesforce instance URL, used to bind the source to the intended organization.
5. Select **Connect**.
6. Leave the source enabled.
7. Choose the desired sync interval and save it.

The source configuration page is shown in [Omni Salesforce source settings](./omni-salesforce-source-settings.png).

The organization-level integration list should show Salesforce as an enabled source with **Settings** and **MCP OAuth** controls, as shown in [Omni Integrations](./omni-integrations-salesforce.png).

## 6. Configure the source-scoped MCP OAuth client

MCP OAuth is configured separately from the native JWT source credentials:

1. From the Salesforce source settings page, select **Configure MCP OAuth client**.
2. Enter the External Client App's **Client ID**.
3. Enter the External Client App's **Client Secret**.
4. Leave **DCR initial access token** blank when using the pre-created External Client App.
5. Save the configuration.

The MCP client configuration is stored for this Salesforce source only. It is not an org-wide credential and is not shared with other Salesforce sources.

Salesforce Dynamic Client Registration is an alternative, but it requires an administrator-issued initial access token. It was not used for this setup.

## 7. Authorize a user's Salesforce MCP access

Each Omni user who uses Salesforce MCP tools must authorize Salesforce separately:

1. Start an MCP action from an Omni chat, or use the Salesforce authorization action when Omni presents it.
2. Follow the Salesforce OAuth redirect.
3. Sign in as the intended Salesforce user.
4. Complete any Salesforce identity verification or MFA/email verification requested by Salesforce.
5. Approve the requested scopes.
6. Return to Omni through the callback.

The resulting credential is stored for the combination of the Omni user and Salesforce source. It is not replaced by the native integration user's JWT credential.

If the per-user credential is missing, revoked, expired, or belongs to another Salesforce organization, MCP access should fail closed and request authorization again.

## 8. Validate the setup

### Validate MCP

Use an Omni chat with a prompt that explicitly requests Salesforce MCP rather than indexed search. The official server requires the tool call to identify a Salesforce username or alias, so the normal sequence is:

1. Discover/load the Salesforce MCP tool.
2. Call `get_username` to resolve the target Salesforce username.
3. Call `run_soql_query` with the requested SOQL.

For example:

```text
Use the Salesforce MCP run_soql_query tool, not indexed search, to run exactly SELECT Id, Name FROM Account LIMIT 5 and list the accounts.
```

A successful MCP request should return data using the authorized user's Salesforce permissions. MCP startup can take several seconds because the official Salesforce MCP server is launched as an isolated stdio subprocess.

### Validate native sync

From **Admin → Settings → Integrations**:

1. Select **Sync** for Salesforce.
2. Open the Salesforce source's sync history.
3. Confirm the run completes and records are indexed.
4. Verify that accounts, contacts, opportunities, leads, cases, and tasks are present as expected.

MCP success does not prove native synchronization works. In the test org, native sync continued to fail its connection check with:

```text
sObject type 'Account' is not supported
```

That indicates a Salesforce-side object availability, license, profile, or permission issue for the JWT integration user. Resolve that separately and rerun the native sync.

## Authentication model

| Capability | Credential | Scope |
| --- | --- | --- |
| Native sync | Salesforce JWT using the integration user and private key | Salesforce source/org |
| Existing native Salesforce actions | Org credential, according to the action's authorization policy | Salesforce source/org |
| MCP tools | Salesforce authorization-code/PKCE OAuth credential | Omni user + Salesforce source |

The org JWT is never an MCP fallback. This separation ensures MCP actions respect the Salesforce identity and permissions of the Omni user who invoked them.

## Security checklist

- Never commit `privatekey.pem`, consumer secrets, access tokens, refresh tokens, authorization codes, passwords, or MFA codes.
- Do not paste secrets into chat or issue trackers.
- Do not capture or publish the Consumer Key and Secret page.
- Redact callback URLs, email addresses, hostnames, and user identifiers from screenshots when sharing this guide externally.
- Use HTTPS for non-local OAuth callbacks.
- Keep the Salesforce integration user's permissions limited to the records and objects Omni must sync.
- Revoke and recreate credentials if a secret is exposed.
