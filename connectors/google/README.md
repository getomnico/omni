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

A plain service account (no DWD and no `sub` impersonation) syncs selected
shared drives with their inherited ACLs. It requires only the
`drive.readonly` scope and shared-drive membership; it does not require DWD,
Admin Directory access, or Workspace administrator access. Group membership
enrichment is optional and can be enabled separately with an Admin Directory
group-read scope and Workspace domain.

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

- Uses only `drive.readonly` by default; source config selects the mode with
  `auth_mode: "service_account_direct"` and one or more
  `folder_path_filters` entries of kind `shared_drive_root`. A domain is not
  required.
- Documents carry the drive-level member list as permissions
  (`public`/`users`/`groups`); the SA's own email is excluded.
- ACLs are fingerprinted each run; a membership change triggers a full drive
  re-traversal so existing documents get the new ACLs.
- Polling only — no webhook registration. When optional group enrichment is
  configured, Google Groups on the drive map to `groups` and memberships are
  emitted through Admin SDK group-membership events during sync. Without it,
  documents granted solely through a Google Group may be underexposed; this is
  fail-closed underexposure, not unauthorized overexposure.
- Per-file overrides outside the drive are not resolved; drive-level ACLs
  apply. Switching an existing DWD source to SA-direct is not supported —
  create a new source.

The setup dialog has two tabs (DWD | Shared drive no-DWD); SA-direct is
Drive-only and validates the SA's role on every selected drive before the source
is created. Group enrichment is best-effort and is not required for onboarding.
The drive settings page preserves any existing optional domain and group scopes
when saving.

Personal Google OAuth connections are configured from **My Integrations**.
The owner must choose either the whole Drive or one or more accessible folders
before indexing starts. Folder selections are stored in
`sources.config.folder_path_filters` and enforced during OAuth sync; Google
still grants the connector its normal read-only Drive OAuth scope.
