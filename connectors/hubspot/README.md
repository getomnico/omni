# HubSpot Connector for Omni

A connector that syncs HubSpot CRM data into Omni.

## Supported Objects

### CRM Core
- Contacts
- Companies
- Deals
- Tickets

### CRM Activities
- Calls
- Emails
- Meetings
- Notes
- Tasks

## Configuration

### Credentials

The connector requires a HubSpot access token (OAuth or Private App):

```json
{
  "access_token": "pat-na1-xxxx-xxxx-xxxx"
}
```

### Source Config (Optional)

```json
{
  "portal_id": "12345678"
}
```

## Required OAuth Scopes

- `crm.objects.contacts.read` (contacts + engagements: calls, emails, meetings, notes, tasks)
- `crm.objects.companies.read`
- `crm.objects.deals.read`
- `tickets`

## Usage

```bash
export CONNECTOR_MANAGER_URL=http://localhost:8080
python main.py
```

## Development

```bash
# Install dependencies
pip install -e ".[dev]"

# Run tests
pytest tests/ -v

# Type check
mypy hubspot_connector/

# Lint
ruff check hubspot_connector/
```
