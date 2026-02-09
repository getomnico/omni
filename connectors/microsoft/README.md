# Microsoft 365 Connector for Omni

Syncs data from Microsoft 365 into Omni via the Microsoft Graph API.

## Supported Services

- **OneDrive** — User drive files
- **Outlook Mail** — Inbox messages
- **Outlook Calendar** — Calendar events
- **SharePoint** — Site document libraries

## Authentication

Uses app-only (client credentials) authentication with Microsoft Entra ID. Required credentials:

- `tenant_id` — Azure AD tenant ID
- `client_id` — Application (client) ID
- `client_secret` — Client secret value

### Required Application Permissions (admin consent)

| Service | Permission | Type |
|---------|-----------|------|
| OneDrive/SharePoint | `Files.Read.All` | Application |
| Outlook Mail | `Mail.Read` | Application |
| Outlook Calendar | `Calendars.Read` | Application |
| SharePoint Sites | `Sites.Read.All` | Application |
| User enumeration | `User.Read.All` | Application |

## Source Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `services` | `list[str]` | All | Which services to sync: `onedrive`, `mail`, `calendar`, `sharepoint` |
| `calendar_past_months` | `int` | 6 | How many months of past events to sync |
| `calendar_future_months` | `int` | 6 | How many months of future events to sync |

