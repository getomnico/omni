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
    /* Markdown / article content */
    h1, h2, h3, h4, h5, h6 { margin: 1em 0 0.5em; line-height: 1.3; }
    h1 { font-size: 1.6em; }
    h2 { font-size: 1.35em; }
    h3 { font-size: 1.15em; }
    p, ul, ol, dl, blockquote { margin: 0.6em 0; }
    ul, ol { padding-left: 1.6em; }
    blockquote {
        margin: 0.8em 0;
        padding: 0.1em 1em;
        border-left: 3px solid #e5e7eb;
        color: #4b5563;
    }
    code { font: 0.9em ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; background: #f3f4f6; padding: 0.15em 0.35em; border-radius: 4px; }
    pre code { display: block; padding: 0.8em 1em; overflow-x: auto; }
    a { color: #2563eb; }
    hr { border: 0; border-top: 1px solid #e5e7eb; margin: 1.2em 0; }
`

export function srcdocDocument(bodyHtml: string): string {
    return `<!doctype html><html><head><meta charset="utf-8"><style>${BASE_CSS}</style></head><body>${bodyHtml}</body></html>`
}
