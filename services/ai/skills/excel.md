# Excel Skill

## Inspection
After fetching an Excel file with `read_document`, run `excel inspect <file>` first via `run_bash`.
This shows: sheet names, dimensions, merged cell regions, detected header row, and a data preview.
Review the output carefully before writing any processing code.

## Data Boundary Detection
Excel sheets often have metadata, titles, or blank rows above the actual data table.
Do NOT assume row 1 is the header. Look for these signals:
- Title/metadata rows: single cell with text spanning merged columns, followed by blank rows
- Header row: the first row where most cells are non-empty and contain short text labels
- Data start: the row immediately after the header
- Data end: last consecutive non-empty row (ignore trailing blanks or footnotes)
- Blank columns on the left: skip leading empty columns to find the actual data range

Use `excel headers <file>` for auto-detected headers per sheet. If detection looks wrong,
use `excel rows <file> <sheet> 1:20` to see raw top rows and determine the real header row.

## Merged Cell Handling
Merged cells are common in headers (e.g., "Revenue" spanning B-D as a group header with
"Jan", "Feb", "Mar" in the row below). They also appear in data rows for grouping.

The `excel` CLI automatically fills merged regions with the top-left cell's value.

When writing openpyxl code yourself:
- Access merged ranges via `ws.merged_cells.ranges`
- A merged cell's value lives only in the top-left cell; others return None
- Unmerge and forward-fill before reading data:
  ```python
  for merge in list(ws.merged_cells.ranges):
      val = ws.cell(merge.min_row, merge.min_col).value
      ws.unmerge_cells(str(merge))
      for r in range(merge.min_row, merge.max_row + 1):
          for c in range(merge.min_col, merge.max_col + 1):
              ws.cell(r, c).value = val
  ```
- Multi-level headers (merged group headers + sub-headers below): read both rows and
  combine them, e.g., "Revenue | Jan" becomes column name "Revenue - Jan"

## Data Type Inference
Before processing, understand cell types:
- Dates: check `cell.is_date` or `cell.number_format`. Dates may appear as serial numbers
  (e.g., 45678) — convert using Excel epoch (1899-12-30).
- Numbers stored as text: cells may contain "1,234.56" as a string. Strip commas before converting.
- Currency: look for currency symbols ($, EUR, GBP) or accounting format. Strip symbols for numeric operations.
- Percentages: may be stored as 0.15 (actual value) or "15%" (string). Check the number format.
- Mixed types in a column: if a column has both numbers and text, treat it as text.

Use `excel schema <file>` to see detected types, sample values, and null counts per column.

## Tool Choice
- Quick lookups (specific cells, ranges, text search): `excel` CLI via `run_bash`
- Data analysis, aggregation, pivoting: `pandas` via `run_python`
- Cell-level editing, formatting, formulas: `openpyxl` via `run_python`
- Always use `present_artifact` after generating or modifying a spreadsheet so the user can download it.

## excel CLI Reference
```
excel inspect <file>                                   — sheets, dims, merged cells, headers, preview
excel headers <file> [sheet]                           — detected header row per sheet
excel schema <file> [sheet]                            — column names, types, sample values, null counts
excel rows <file> <sheet> <range>                      — specific rows as TSV (e.g., 100:110)
excel cell <file> <sheet> <range>                      — specific cell/range (e.g., J101 or A1:D10)
excel grep <file> <pattern> [--sheet S] [--column C]   — search for text across cells
excel filter <file> <sheet> <expr>                     — filter rows (pandas query syntax)
excel to-csv <file> [--sheet S]                        — export as CSV (merged cells filled)
```
