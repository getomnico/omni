export const GENERATED_HTML_ARTIFACT_CSP = [
    "default-src 'none'",
    "base-uri 'none'",
    "script-src 'unsafe-inline'",
    "style-src 'unsafe-inline'",
    "img-src 'none' data: blob:",
    "font-src 'none'",
    "connect-src 'none'",
    "media-src 'none'",
    "object-src 'none'",
    "form-action 'none'",
    "frame-ancestors 'self'",
    "navigate-to 'none'",
    'sandbox allow-scripts allow-downloads',
].join('; ')

export function artifactResponseHeaders(
    contentType: string,
    cacheControl: string,
): Record<string, string> {
    const headers: Record<string, string> = {
        'Content-Type': contentType,
        'Cache-Control': cacheControl,
        'X-Content-Type-Options': 'nosniff',
        'Referrer-Policy': 'no-referrer',
        // Artifact previews use opaque iframe origins and do not need CORS
        // access. This header also preserves the existing download behavior.
        'Access-Control-Allow-Origin': '*',
    }
    if (contentType.toLowerCase().startsWith('text/html')) {
        headers['Content-Security-Policy'] = GENERATED_HTML_ARTIFACT_CSP
    }
    return headers
}
