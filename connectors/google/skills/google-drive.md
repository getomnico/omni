# Google Drive Skill

Source: adapted from `googleworkspace/cli` `skills/gws-drive/SKILL.md`
at https://github.com/googleworkspace/cli. Omni routes Google Workspace CLI
requests through connector actions instead of direct local `gws` commands.

## Tool Use
Use the Google connector's `google_workspace_schema` and
`google_workspace_call` actions for Drive API access. Do not run local `gws` commands from the sandbox for Drive access; Omni owns authentication and routes
requests through connector tools with source permissions.

For unfamiliar Drive or editor methods, call `google_workspace_schema` first
with schema names like `drive.files.list`, `drive.files.get`,
`drive.permissions.create`, `docs.documents.batchUpdate`,
`sheets.spreadsheets.values.update`, or `slides.presentations.batchUpdate`.
Leave `resolve_refs` unset/false by default. Google discovery schemas are a type
graph, and recursively resolving all references can produce multi-MB schemas.
When a schema contains `$ref` values, fetch only the specific referenced type you
need, e.g. `sheets.Request`, `sheets.UpdateCellsRequest`, `sheets.CellData`,
`docs.Request`, or `docs.InsertTextRequest`. Use `resolve_refs: true` only for
small schemas or when targeted type lookups are not enough.

Then call `google_workspace_call` with the matching `service` (`drive`, `docs`,
`sheets`, or `slides`), the resource path such as `files`, `permissions`,
`documents`, `spreadsheets.values`, or `presentations`, the method, and any
query parameters in `params`.

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

For file metadata updates, use Drive methods and send only fields that should
change. Many Drive operations use patch semantics, and omitted fields should
remain untouched.

For Google Docs content edits, use the Docs API via `google_workspace_call`,
e.g. `service: "docs"`, `resource: "documents"`, `method: "batchUpdate"`,
`params: {"documentId": "..."}`, and a JSON body with `requests`.

For Google Sheets content edits, use the Sheets API via `google_workspace_call`,
e.g. `service: "sheets"`, `resource: "spreadsheets.values"`, `method:
"update"`, `params` containing `spreadsheetId`, `range`, and
`valueInputOption`, plus a JSON body with `values`. Use
`sheets.spreadsheets.batchUpdate` for structural changes.

For spreadsheet tasks, choose between export-and-analyze and API edits:
- If the task is reading, analyzing, summarizing, transforming, or creating a
  new workbook based on one or more existing sheets, exporting/fetching the
  spreadsheet as `.xlsx` and working with it in the sandbox using Python is often
  simpler and more reliable.
- If the task is a specific in-place edit to an existing sheet, such as changing
  cells, adding/removing rows or columns, adding a pivot table, creating a chart,
  formatting ranges, or changing sheet structure, use the Sheets API. Prefer
  `spreadsheets.values.update`/`append` for cell values and
  `spreadsheets.batchUpdate` for structural or formatting changes.

For Google Slides content edits, use the Slides API via `google_workspace_call`,
e.g. `service: "slides"`, `resource: "presentations"`, `method:
"batchUpdate"`, `params: {"presentationId": "..."}`, and a JSON body with
`requests`.
