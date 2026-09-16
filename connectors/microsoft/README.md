# Microsoft 365 Connector for Omni

Syncs data from Microsoft 365 into Omni via the Microsoft Graph API.

## Supported Services

- **OneDrive** — User drive files
- **Outlook Mail** — Inbox messages
- **Outlook Calendar** — Calendar events
- **SharePoint** — Site document libraries

## Agent Actions

The connector exposes actions that let agents work with Outlook Calendar
interactively (not just search the synced index):

| Action | Mode | Description |
|--------|------|-------------|
| `list_events` | read | List the caller's calendar events in a time range (defaults to the next 7 days) |
| `create_event` | write | Create an event on the caller's calendar (attendees, location, body, Teams meeting) |

Actions execute with per-user delegated tokens, so each user must connect
their own Microsoft account for these tools to become available.

## Authentication

Uses app-only (client credentials) authentication with Microsoft Entra ID. Required credentials:

- `tenant_id` — Azure AD tenant ID
- `client_id` — Application (client) ID
- `client_secret` — Client secret value

Also register the per-user OAuth callback under **Authentication → Add a
platform → Web** (the setup dialog shows the exact URL for the instance;
it is `<APP_URL>/api/oauth/callback`). Without it, the "Connect account"
consent screen fails with `AADSTS500113`.

### Required Application Permissions (admin consent)

Application permissions drive org-wide sync. Each of them must be added
under **API permissions** and granted **admin consent** — an app
registration without granted consent authenticates fine but every Graph
call fails with `Authorization_RequestDenied` ("Insufficient privileges to
complete the operation").

| Service | Permission | Type |
|---------|-----------|------|
| OneDrive/SharePoint | `Files.Read.All` | Application |
| Outlook Mail | `Mail.Read` | Application |
| Outlook Calendar | `Calendars.Read` | Application |
| SharePoint Sites | `Sites.Read.All` | Application |
| User enumeration | `User.Read.All` | Application |
| Teams Chats | `Chat.Read.All` | Application |
| Teams Chat Messages | `ChatMessage.Read.All` | Application |

### Delegated Permissions (per-user tools)

Delegated permissions are used when individual users connect their account
to get agent actions. They don't need admin consent for a single-tenant
app, but users can only consent to non-admin-scoped permissions:

| Purpose | Permission |
|---------|-----------|
| Sign-in / identity | `User.Read` |
| Calendar actions (`list_events`) | `Calendars.Read` |
| Calendar actions (`create_event`) | `Calendars.ReadWrite` |

## Source Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `services` | `list[str]` | All | Which services to sync: `onedrive`, `mail`, `calendar`, `sharepoint` |
| `calendar_past_months` | `int` | 6 | How many months of past events to sync |
| `calendar_future_months` | `int` | 6 | How many months of future events to sync |

