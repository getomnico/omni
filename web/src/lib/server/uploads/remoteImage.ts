// Server-side fetch for images pasted as remote URLs (Safari web-image
// pastes expose only text/html, never files). The URL comes from user
// content, so this is a classic SSRF foothold: validate scheme, resolve
// every DNS address and reject private/loopback targets, follow redirects
// manually re-validating each hop, cap size, and require an image type.

import { lookup } from 'node:dns/promises'
import { isIP } from 'node:net'

const ALLOWED_REDIRECTS = 3
const FETCH_TIMEOUT_MS = 15_000
const MAX_IMAGE_BYTES = 10_000_000

export class UrlFetchError extends Error {
    constructor(
        message: string,
        readonly status: number = 400,
    ) {
        super(message)
    }
}

export function isPublicIp(ip: string): boolean {
    if (isIP(ip) === 4) {
        const [a, b] = ip.split('.').map(Number)
        if (a === 0 || a === 10 || a === 127) return false
        if (a === 169 && b === 254) return false
        if (a === 172 && b >= 16 && b <= 31) return false
        if (a === 192 && b === 168) return false
        if (a === 100 && b >= 64 && b <= 127) return false
        return true
    }
    if (isIP(ip) === 6) {
        const lower = ip.toLowerCase()
        if (lower === '::' || lower === '::1') return false
        if (lower.startsWith('fe80')) return false
        if (lower.startsWith('fc') || lower.startsWith('fd')) return false
        if (lower.startsWith('::ffff:')) return isPublicIp(lower.slice(7))
        return true
    }
    return false
}

async function assertResolvesPublic(hostname: string): Promise<void> {
    if (isPublicIp(hostname)) return
    let addresses
    try {
        addresses = await lookup(hostname, { all: true, verbatim: true })
    } catch {
        throw new UrlFetchError('Pasted image host could not be resolved')
    }
    if (addresses.length === 0 || addresses.some((a) => !isPublicIp(a.address))) {
        throw new UrlFetchError('Pasted image host is not a public address')
    }
}

function assertHttpUrl(value: string): URL {
    let url: URL
    try {
        url = new URL(value)
    } catch {
        throw new UrlFetchError('Pasted image URL is malformed')
    }
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        throw new UrlFetchError('Pasted image URL must be http(s)')
    }
    return url
}

async function openImageResponse(rawUrl: string): Promise<Response> {
    let url = assertHttpUrl(rawUrl)
    for (let hop = 0; hop <= ALLOWED_REDIRECTS; hop++) {
        await assertResolvesPublic(url.hostname)
        const resp = await fetch(url, {
            redirect: 'manual',
            signal: AbortSignal.timeout(FETCH_TIMEOUT_MS),
            headers: { accept: 'image/*' },
        })
        if (resp.status >= 300 && resp.status < 400) {
            const location = resp.headers.get('location')
            resp.body?.cancel()
            if (!location) throw new UrlFetchError('Pasted image redirect had no target')
            if (hop === ALLOWED_REDIRECTS) {
                throw new UrlFetchError('Too many redirects fetching pasted image')
            }
            url = assertHttpUrl(new URL(location, url).href)
            continue
        }
        return resp
    }
    throw new UrlFetchError('Unreachable')
}

export async function fetchRemoteImageBytes(
    rawUrl: string,
): Promise<{ bytes: Buffer; contentType: string }> {
    const resp = await openImageResponse(rawUrl)
    if (!resp.ok) {
        resp.body?.cancel()
        throw new UrlFetchError('Pasted image URL returned an error', 502)
    }
    const contentType = (resp.headers.get('content-type') || '').split(';')[0].trim()
    if (!contentType.startsWith('image/')) {
        resp.body?.cancel()
        throw new UrlFetchError('Pasted URL is not an image', 415)
    }
    const buffer = Buffer.from(await resp.arrayBuffer())
    if (buffer.length === 0) throw new UrlFetchError('Pasted image is empty', 502)
    if (buffer.length > MAX_IMAGE_BYTES) {
        throw new UrlFetchError('Pasted image is too large', 413)
    }
    return { bytes: buffer, contentType }
}
