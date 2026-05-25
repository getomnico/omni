# Google Drive Skill

Source: adapted from `googleworkspace/cli` `skills/gws-drive/SKILL.md`
at https://github.com/googleworkspace/cli. Omni uses Google connector MCP tools
instead of direct local `gws` commands.

## Tool Use
Use available connector tools for Google Drive, usually named with the
`google_drive__` prefix. Do not run local `gws` commands from the sandbox for
Drive access; Omni owns authentication and routes requests through connector
tools with source permissions.

## Search First
When the user gives a file name, folder name, shared drive name, or vague
description, search/list first and inspect candidate metadata before reading or
modifying anything.

Prefer stable IDs from tool results over names. Names are not unique in Drive.
When multiple candidates match, compare path, owner, MIME type, modified time,
shared drive, and trashed status before choosing.

## Reading Files
For Google Workspace files, prefer export/read tools that return text or common
portable formats. For binary files, use metadata first to confirm MIME type and
size before fetching bytes.

When only part of a file is needed, ask for the smallest useful content or
metadata slice. Avoid loading large files blindly.

## Drive Query Hints
Use Drive query filters when available:
- Exclude trash unless the user asks for deleted files.
- Filter by MIME type for Docs, Sheets, Slides, PDFs, folders, or shared drives.
- Use modified time ranges for recent-file requests.
- Use parent/folder IDs once a folder has been identified.

## Sharing And Permissions
Treat permission-changing tools as write actions. Before adding, updating, or
removing permissions, confirm the target file ID, the grantee, the role, and
whether the file is in a shared drive.

Concurrent permission updates on the same file can race; batch decisions in one
operation where the tool supports it, or perform changes sequentially.

## Uploads And Updates
Before creating, copying, moving, renaming, or uploading files, verify the
destination folder or shared drive. Use dry-run or preview behavior when an
available tool supports it.

For updates, send only fields that should change. Many Drive operations use
patch semantics, and omitted fields should remain untouched.
