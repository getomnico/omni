import { assertRemoteMcpDestinationAllowed, validateRemoteMcpUrl } from '$lib/server/mcp/client'

function errorDetail(err: unknown): string {
    if (err && typeof err === 'object') {
        const body = (err as { body?: unknown }).body
        if (typeof body === 'string' && body) return body
        if (body && typeof body === 'object' && 'message' in body) {
            const message = (body as { message?: unknown }).message
            if (typeof message === 'string' && message) return message
        }
    }
    return err instanceof Error && err.message ? err.message : 'invalid URL'
}

/// Validate an admin-entered Windshift server URL (public or internal) using
/// the same SSRF policy as remote MCP sources: http(s) only, no credentials
/// or fragment, and the resolved address must not be private or reserved.
/// Empty strings are allowed (the internal URL is optional). Throws an Error
/// with a user-facing message when the URL is rejected.
export async function validateWindshiftServerUrl(label: string, url: string): Promise<void> {
    const trimmed = url.trim()
    if (!trimmed) return
    try {
        const parsed = validateRemoteMcpUrl(trimmed)
        await assertRemoteMcpDestinationAllowed(parsed)
    } catch (err) {
        throw new Error(`${label} is not allowed: ${errorDetail(err)}`)
    }
}
