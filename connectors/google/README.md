# Google Connector

Indexes Google Workspace content into Omni: Google Drive, Gmail, and Google
Chat. Configuration is managed through the Omni admin UI at
`/admin/settings/integrations`.

## Authentication

The connector supports two auth modes, selected per source in the setup UI:

### Domain-wide delegation (default)

The existing flow: a service account with domain-wide delegation impersonates
Workspace users (`sub` claim) and uses the Admin SDK for user/group discovery.
Requires admin email, domain, and DWD scopes. Used for org-wide Drive, Gmail,
and Chat sync.

### Service account direct (shared drives, no DWD)

A plain service account (no DWD, no Admin SDK scopes, no `sub` impersonation)
syncs selected shared drives with their inherited ACLs.

Google-side setup:

1. Create a service account key (IAM & Admin → Service Accounts → Keys).
2. In each shared drive → **Manage members**, add the service account email
   (`<name>@<project>.iam.gserviceaccount.com`) as **Content manager**
   (`fileOrganizer`) or **Manager** (`organizer`) — required, fail-closed.
3. Verify your Workspace external-sharing policy permits adding
   `*.iam.gserviceaccount.com` to shared drives. The SA is an external
   principal, so a scoped OU / trust-rule exception may be needed. If the
   policy cannot allow the SA at all, use OAuth or DWD with an internal user
   instead.

Behavior:

- Only `drive.readonly` scope; source config selects the mode with
  `auth_mode: "service_account_direct"` and one or more
  `folder_path_filters` entries of kind `shared_drive_root`.
- Documents carry the drive-level member list as permissions
  (`public`/`users`/`groups`); the SA's own email is excluded.
- ACLs are fingerprinted each run; a membership change triggers a full drive
  re-traversal so existing documents get the new ACLs.
- Polling only — no webhook registration. Google Groups on the drive map to
  `groups` but group members are undergranted in v1 (group membership comes
  from Admin SDK sync, unavailable here). Prefer drives with direct
  user/domain/anyone members.
- Per-file overrides outside the drive are not resolved; drive-level ACLs
  apply. Switching an existing DWD source to SA-direct is not supported —
  create a new source.

The setup dialog has two tabs (DWD | Shared drive no-DWD); SA-direct is
Drive-only and validates the SA's role on every selected drive before the
source is created. The drive settings page repeats that validation before
Save.
