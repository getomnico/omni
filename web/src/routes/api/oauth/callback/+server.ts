import { error, redirect } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { ulid } from 'ulid'
import { exchangeCodeAndIdentify } from '$lib/server/oauth/connectorOAuth'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { logger } from '$lib/server/logger'
import {
    getActiveSourcesByTypeAndScope,
    getActiveSourcesByTypeAndOwner,
} from '$lib/server/db/sources'

const SOURCE_NAMES: Record<string, string> = {
    google_drive: 'Google Drive (OAuth)',
    gmail: 'Gmail (OAuth)',
}

/// Unified OAuth callback. Provider-agnostic — dispatches based on the flow
/// stored in the OAuth state.
export const GET: RequestHandler = async ({ url, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const code = url.searchParams.get('code')
    const stateToken = url.searchParams.get('state')
    const oauthError = url.searchParams.get('error')

    if (oauthError) {
        logger.error('OAuth provider error', { error: oauthError })
        throw redirect(302, '/settings/integrations?error=oauth_denied')
    }
    if (!code || !stateToken) {
        throw error(400, 'Missing code or state')
    }

    let exchange
    try {
        exchange = await exchangeCodeAndIdentify(code, stateToken)
    } catch (err) {
        logger.error('OAuth exchange failed', { err: String(err) })
        throw redirect(302, '/settings/integrations?error=oauth_failed')
    }

    const { tokens, state, principalEmail, config } = exchange

    if (state.user_id !== locals.user.id) {
        throw error(403, 'OAuth state does not match the signed-in user')
    }
    if (!state.metadata) {
        throw error(400, 'OAuth state has no metadata')
    }

    const flow = state.metadata.flow
    const grantedScopes = (tokens.scope ?? '').split(config.scope_separator).filter(Boolean)
    const requiredScopes = state.metadata.requiredScopes
    if (state.metadata.strictScopeCheck) {
        const missing = requiredScopes.filter((s) => !grantedScopes.includes(s))
        if (missing.length > 0) {
            return userWriteResultPage({
                ok: false,
                sourceId: flow.type === 'user_write' ? flow.sourceId : null,
                message: `Missing required scopes: ${missing.join(', ')}`,
            })
        }
    }

    const expiresAt = tokens.expires_in ? new Date(Date.now() + tokens.expires_in * 1000) : null
    const credentials = {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token ?? null,
        token_type: tokens.token_type ?? 'Bearer',
    }

    if (flow.type === 'user_write') {
        await serviceCredentialsRepository.createForUser({
            sourceId: flow.sourceId,
            userId: locals.user.id,
            provider: config.provider,
            authType: 'oauth',
            principalEmail,
            credentials,
            config: { granted_scopes: grantedScopes },
            expiresAt,
        })
        return userWriteResultPage({ ok: true, sourceId: flow.sourceId, returnTo: flow.returnTo })
    }

    // connect_source flow: for each requested source_type, attach to existing
    // org source if there is one; otherwise create a personal source.
    for (const sourceType of flow.sourceTypes) {
        const [orgSource] = await getActiveSourcesByTypeAndScope(sourceType, 'org')

        if (orgSource) {
            await serviceCredentialsRepository.createForUser({
                sourceId: orgSource.id,
                userId: locals.user.id,
                provider: config.provider,
                authType: 'oauth',
                principalEmail,
                credentials,
                config: { granted_scopes: grantedScopes },
                expiresAt,
            })
            logger.info(
                `Attached per-user OAuth creds to org source ${orgSource.id} (${sourceType}) for user ${locals.user.id}`,
            )
            continue
        }

        const [existing] = await getActiveSourcesByTypeAndOwner(sourceType, locals.user.id)

        if (existing) {
            // Source already exists for this user — refresh its creds in place.
            await serviceCredentialsRepository.createForUser({
                sourceId: existing.id,
                userId: locals.user.id,
                provider: config.provider,
                authType: 'oauth',
                principalEmail,
                credentials,
                config: { granted_scopes: grantedScopes },
                expiresAt,
            })
            continue
        }

        const [newSource] = await db
            .insert(sources)
            .values({
                id: ulid(),
                name: SOURCE_NAMES[sourceType] || sourceType,
                sourceType,
                scope: 'user',
                config: {},
                createdBy: locals.user.id,
                isActive: true,
            })
            .returning()

        await serviceCredentialsRepository.createForUser({
            sourceId: newSource.id,
            userId: locals.user.id,
            provider: config.provider,
            authType: 'oauth',
            principalEmail,
            credentials,
            config: { granted_scopes: grantedScopes },
            expiresAt,
        })

        logger.info(
            `Created personal source ${newSource.id} (${sourceType}) for user ${locals.user.id}`,
        )
    }

    throw redirect(302, flow.returnTo ?? '/settings/integrations?success=connected')
}

function userWriteResultPage(opts: {
    ok: boolean
    sourceId: string | null
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
