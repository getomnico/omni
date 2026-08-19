# Salesforce Connector

Index accounts, contacts, opportunities, leads, cases, and tasks from Salesforce
into Omni, with a faithful mirror of Salesforce's sharing model.

## Sync modes

- **full** — scans every configured object (keyset-paginated by `Id`), emits all
  documents along with org users, groups, and per-record sharing grants.
- **incremental** — delta scan per object (`SystemModstamp` watermark with a
  keyset cursor and a look-back overlap window), plus `GetDeleted` tombstones.
- **realtime** — a long-lived polling sync (supervised by the connector-manager)
  that picks up created/updated/deleted records on a short interval.

## Checkpointing & resumability

Per-object keyset cursors (`record_cursors`) let an interrupted scan resume
mid-object. Incremental watermarks are the max `SystemModstamp` per object. A
schema fingerprint (object list + fields + API version) is stored in
`connector_state`; when it changes, watermarks are invalidated and a full resync
is forced so newly configured fields/objects are always indexed.

## Permissions

Salesforce visibility is mirrored faithfully:

- **Owners** — the record owner (user) is granted `permissions.users`.
- **Queues** — queue-owned records are granted to `permissions.groups` via the
  queue's group membership.
- **Role hierarchy** — with "Grant Access Using Hierarchies" (default), a record
  is visible to the owner's role and every ancestor role; the role groups are
  emitted so membership stays current.
- **Sharing rules / manual shares** — rows in the per-object `*Share` tables
  grant users, public groups, or roles (role shares include roles below them).
- **Org-wide defaults** — objects with public-read defaults (Account/Contact by
  default, configurable) are emitted as `public: true`.
- **Users & groups** — active users are emitted as `person_sync` events
  (people directory); public groups, queues, and roles as
  `group_membership_sync` events.

Group and role emails are synthesized from Salesforce ids
(`{id}@salesforce.groups` / `...@salesforce.roles`) because Salesforce groups
have no email addresses of their own.

## Search operators

`owner:`, `status:`, `priority:`, `stage:`, `account:`, `industry:`,
`lead_source:` — matching the `attributes` emitted on each document.

## Actions

- `find_records`, `get_case` (read)
- `create_case`, `update_case_status`, `create_task`, `update_task_status` (write)

## Credentials

The connector expects `access_token` and `instance_url` (Bearer token, e.g. from
a Connected App session). Configure the source from the admin integrations page
with the org's instance URL and a valid access token.

## Development

```bash
uv sync --dev
uv run pytest tests -m "not integration"       # unit tests
uv run pytest tests                            # integration tests (Docker)
uv run ruff check . && uv run mypy salesforce_connector
```
