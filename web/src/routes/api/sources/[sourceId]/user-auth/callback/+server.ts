import { error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { GoogleConnectorOAuthService } from '$lib/server/oauth/googleConnector'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { logger } from '$lib/server/logger'

/// Per-user OAuth callback. Exchanges the code for tokens, validates the user
/// granted the required write scopes, and writes a per-user
/// service_credentials row attached to the org-wide source.
export const GET: RequestHandler = async ({ params, locals, url }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = params.sourceId
    if (!sourceId) {
        throw error(400, 'sourceId is required')
    }

    const code = url.searchParams.get('code')
    const stateToken = url.searchParams.get('state')
    const oauthError = url.searchParams.get('error')

    if (oauthError) {
        return successPage({
            ok: false,
            sourceId,
            message: `Authorization denied: ${oauthError}`,
        })
    }
    if (!code || !stateToken) {
        throw error(400, 'Missing code or state')
    }

    const [source] = await db
        .select({
            id: sources.id,
            sourceType: sources.sourceType,
            scope: sources.scope,
            isDeleted: sources.isDeleted,
        })
        .from(sources)
        .where(and(eq(sources.id, sourceId), eq(sources.isDeleted, false)))
        .limit(1)

    if (!source) {
        throw error(404, 'Source not found')
    }
    if (source.scope !== 'org') {
        throw error(400, 'Per-user authentication only applies to org-wide sources')
    }

    const sourceType = source.sourceType
    if (sourceType !== 'google_drive' && sourceType !== 'gmail') {
        throw error(501, `Per-user OAuth is not implemented for source_type=${sourceType}`)
    }

    let tokens
    let state
    try {
        const result = await GoogleConnectorOAuthService.exchangeUserWriteCode(
            sourceId,
            code,
            stateToken,
        )
        tokens = result.tokens
        state = result.state
    } catch (err) {
        logger.error('user-auth callback token exchange failed', { sourceId, err: String(err) })
        return successPage({
            ok: false,
            sourceId,
            message: 'Failed to exchange OAuth code. Please try again.',
        })
    }

    if (state.user_id !== locals.user.id) {
        throw error(403, 'OAuth state does not match the signed-in user')
    }

    const requiredScopes: string[] = state.metadata?.requiredScopes ?? []
    const grantedScopes = (tokens.scope ?? '').split(' ').filter(Boolean)
    const missing = requiredScopes.filter((s) => !grantedScopes.includes(s))
    if (missing.length > 0) {
        return successPage({
            ok: false,
            sourceId,
            message: `Missing required scopes: ${missing.join(', ')}. Please reconnect and grant all requested permissions.`,
        })
    }

    const principalEmail = await GoogleConnectorOAuthService.fetchUserEmail(tokens.access_token)
    const expiresAt = tokens.expires_in ? new Date(Date.now() + tokens.expires_in * 1000) : null

    await serviceCredentialsRepository.createForUser({
        sourceId,
        userId: locals.user.id,
        provider: 'google',
        authType: 'oauth',
        principalEmail,
        credentials: {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token ?? null,
            token_type: tokens.token_type ?? 'Bearer',
        },
        config: {
            granted_scopes: grantedScopes,
        },
        expiresAt,
    })

    const returnTo = state.metadata?.returnTo as string | undefined
    return successPage({ ok: true, sourceId, returnTo })
}

/// Tiny HTML page that closes the popup/tab and notifies the original tab via
/// BroadcastChannel. Shape matches the approval-card listener (see plan §UX).
function successPage(opts: {
    ok: boolean
    sourceId: string
    message?: string
    returnTo?: string
}): Response {
    const payload = JSON.stringify({
        type: 'omni:user-auth-result',
        ok: opts.ok,
        sourceId: opts.sourceId,
        message: opts.message ?? null,
    })
    const heading = opts.ok ? 'Connected' : 'Connection failed'
    const subhead = opts.ok
        ? 'You can close this window and return to your chat.'
        : (opts.message ?? 'Something went wrong.')
    const returnLink = opts.returnTo
        ? `<p><a href="${escapeHtml(opts.returnTo)}">Return to chat</a></p>`
        : ''

    const html = `<!doctype html>
<html><head><meta charset="utf-8"><title>${heading}</title></head>
<body style="font-family: system-ui, sans-serif; padding: 2rem;">
<h1>${heading}</h1>
<p>${escapeHtml(subhead)}</p>
${returnLink}
<script>
try {
  const ch = new BroadcastChannel('omni-user-auth');
  ch.postMessage(${payload});
  ch.close();
} catch (e) {}
setTimeout(() => { try { window.close(); } catch (e) {} }, 800);
</script>
</body></html>`

    return new Response(html, {
        status: 200,
        headers: { 'Content-Type': 'text/html; charset=utf-8' },
    })
}

function escapeHtml(s: string): string {
    return s
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;')
}
