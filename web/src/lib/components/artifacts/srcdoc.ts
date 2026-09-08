// Builds a standalone HTML document for the sandboxed `srcdoc` iframes used by
// the docx/xlsx/text artifact viewers. Content rendered inside these iframes is
// inert (no scripts allowed), so document conversions never need to execute.
const BASE_CSS = `
    :root { color-scheme: light; }
    body {
        margin: 0;
        padding: 16px;
        font: 14px/1.55 system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
        color: #1f2937;
        background: #ffffff;
    }
    img { max-width: 100%; height: auto; }
    table { border-collapse: collapse; margin: 8px 0; }
    td, th { border: 1px solid #d1d5db; padding: 4px 10px; text-align: left; vertical-align: top; }
    th { background: #f3f4f6; font-weight: 600; }
    pre {
        margin: 0;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
        font: 12px/1.6 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    }
`

export function srcdocDocument(bodyHtml: string): string {
    return `<!doctype html><html><head><meta charset="utf-8"><style>${BASE_CSS}</style></head><body>${bodyHtml}</body></html>`
}
