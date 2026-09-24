import { describe, expect, it } from 'vitest'
import { artifactResponseHeaders, GENERATED_HTML_ARTIFACT_CSP } from './artifacts'

describe('artifact response security', () => {
    it('sandboxes generated HTML without runtime network or form access', () => {
        const headers = artifactResponseHeaders('text/html; charset=utf-8', 'private, max-age=1')
        const csp = headers['Content-Security-Policy']

        expect(csp).toBe(GENERATED_HTML_ARTIFACT_CSP)
        expect(csp).toContain("connect-src 'none'")
        expect(csp).toContain("form-action 'none'")
        expect(csp).toContain("font-src 'none'")
        expect(csp).toContain("frame-ancestors 'self'")
        expect(csp).toContain('sandbox allow-scripts allow-downloads')
        expect(headers['X-Content-Type-Options']).toBe('nosniff')
    })

    it('does not add the HTML CSP to ordinary artifacts', () => {
        const headers = artifactResponseHeaders('application/pdf', 'private, max-age=1')
        expect(headers['Content-Security-Policy']).toBeUndefined()
    })
})
