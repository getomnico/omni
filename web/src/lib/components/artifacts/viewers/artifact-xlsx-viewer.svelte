<script lang="ts">
    import { onMount } from 'svelte'
    import type { ArtifactData } from '$lib/utils/artifacts'
    import { escapeHtml, MAX_XLSX_PREVIEW_BYTES } from '$lib/utils/artifacts'
    import { srcdocDocument } from '../srcdoc'
    import ArtifactViewerFeedback from '../artifact-viewer-feedback.svelte'

    type CellValue = string | number | boolean | Date | null | undefined

    type XlsxSheet = {
        sheet: string
        data: CellValue[][]
    }

    const MAX_PREVIEW_SHEETS = 3
    const MAX_PREVIEW_ROWS = 200
    const MAX_PREVIEW_COLS = 26

    type Props = {
        artifact: ArtifactData
    }

    let { artifact }: Props = $props()

    let doc = $state<string | null>(null)
    let error = $state<string | null>(null)

    function cellText(value: CellValue): string {
        if (value === null || value === undefined) return ''
        if (typeof value === 'string') return value
        if (value instanceof Date) return value.toLocaleString()
        return String(value)
    }

    function buildPreviewHtml(sheets: XlsxSheet[]): string {
        const parts: string[] = []
        for (const sheet of sheets) {
            const rows = sheet.data.slice(0, MAX_PREVIEW_ROWS)
            parts.push(
                `<p style="font-weight:600;margin:16px 0 4px;">${escapeHtml(sheet.sheet)}</p>`,
            )
            if (rows.length === 0) {
                parts.push('<p style="color:#6b7280;">(empty sheet)</p>')
                continue
            }
            parts.push('<table>')
            for (const [rowIndex, row] of rows.entries()) {
                const cells = row.slice(0, MAX_PREVIEW_COLS)
                const tag = rowIndex === 0 ? 'th' : 'td'
                parts.push('<tr>')
                for (const cell of cells) {
                    parts.push(`<${tag}>${escapeHtml(cellText(cell))}</${tag}>`)
                }
                parts.push('</tr>')
            }
            parts.push('</table>')
            const notes: string[] = []
            if (sheet.data.length > MAX_PREVIEW_ROWS) {
                notes.push(`showing first ${MAX_PREVIEW_ROWS} of ${sheet.data.length} rows`)
            }
            if (rows.some((row) => row.length > MAX_PREVIEW_COLS)) {
                notes.push(`first ${MAX_PREVIEW_COLS} columns shown`)
            }
            if (notes.length > 0) {
                parts.push(
                    `<p style="color:#6b7280;font-size:12px;">Preview truncated — ${notes.join('; ')}.</p>`,
                )
            }
        }
        return parts.join('')
    }

    onMount(async () => {
        if (artifact.size_bytes > MAX_XLSX_PREVIEW_BYTES) {
            error = `File is ${Math.round(artifact.size_bytes / (1024 * 1024))} MB; the preview limit is 25 MB.`
            return
        }
        try {
            const response = await fetch(artifact.url)
            if (!response.ok) {
                throw new Error(`Failed to load file (HTTP ${response.status})`)
            }
            const arrayBuffer = await response.arrayBuffer()
            const readXlsxFile = (await import('read-excel-file/browser')).default
            const sheets = (await readXlsxFile(arrayBuffer)) as XlsxSheet[]
            doc = srcdocDocument(buildPreviewHtml(sheets.slice(0, MAX_PREVIEW_SHEETS)))
        } catch (err) {
            error = err instanceof Error ? err.message : 'Could not parse the spreadsheet.'
        }
    })
</script>

{#if doc}
    <iframe title={artifact.title} sandbox="" srcdoc={doc} class="h-full w-full border-0 bg-white"
    ></iframe>
{:else}
    <ArtifactViewerFeedback
        state={error ? 'error' : 'loading'}
        message={error ?? 'Rendering spreadsheet…'}
        {artifact} />
{/if}
