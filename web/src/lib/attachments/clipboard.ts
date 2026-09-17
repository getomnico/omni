// Clipboard-to-File extraction for the chat composer.
//
// Two paste flavors must work everywhere:
// 1. File pastes (screenshots, copied image files) — browsers expose them via
//    clipboardData.items/files during the paste event.
// 2. Web-image pastes (copy image on a page, then paste — Safari's default)
//    — no files are exposed, only text/html with an <img src>. Remote sources
//    cannot be fetched from the page (CORS), so they go through the
//    /api/uploads/from-url proxy, which enforces SSRF guards server-side.

const MAX_REMOTE_IMAGE_BYTES = 10_000_000

export function filesFromClipboardData(data: DataTransfer | null): File[] {
    if (!data) return []
    const files: File[] = []
    for (const item of Array.from(data.items)) {
        if (item.kind !== 'file') continue
        const file = item.getAsFile()
        if (file) files.push(file)
    }
    if (files.length === 0) return Array.from(data.files)
    return files
}

export function imageUrlsFromClipboardHtml(html: string | null): string[] {
    if (!html) return []
    const urls: string[] = []
    const imgRe = /<img\b[^>]*\bsrc\s*=\s*["']?([^"'>\s]+)/gi
    let match: RegExpExecArray | null
    while ((match = imgRe.exec(html)) !== null) {
        const src = match[1]
        if (src && !urls.includes(src)) urls.push(src)
    }
    return urls
}

async function fileFromDataOrBlobUrl(url: string): Promise<File | null> {
    try {
        const resp = await fetch(url)
        if (!resp.ok) return null
        const blob = await resp.blob()
        if (blob.size === 0 || blob.size > MAX_REMOTE_IMAGE_BYTES) return null
        const type = blob.type || 'application/octet-stream'
        const ext = type.split('/')[1]?.replace(/[^a-z0-9]/gi, '') || 'bin'
        return new File([blob], `pasted-image.${ext}`, { type })
    } catch {
        return null
    }
}

async function fileFromRemoteUrl(url: string): Promise<File | null> {
    try {
        const resp = await fetch('/api/uploads/from-url', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ url }),
        })
        if (!resp.ok) return null
        const blob = await resp.blob()
        const type = resp.headers.get('X-Image-Content-Type') || blob.type || 'image/png'
        const ext = type.split('/')[1]?.replace(/[^a-z0-9]/gi, '') || 'png'
        return new File([blob], `pasted-image.${ext}`, { type })
    } catch {
        return null
    }
}

/** Materialize File objects for image URLs found in pasted HTML. */
export async function filesFromClipboardHtml(html: string | null): Promise<File[]> {
    const urls = imageUrlsFromClipboardHtml(html)
    const files: File[] = []
    for (const url of urls) {
        if (url.startsWith('data:') || url.startsWith('blob:')) {
            const file = await fileFromDataOrBlobUrl(url)
            if (file) files.push(file)
        } else if (/^https?:\/\//i.test(url)) {
            const file = await fileFromRemoteUrl(url)
            if (file) files.push(file)
        }
    }
    return files
}
