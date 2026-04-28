import { error, redirect } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { ulid } from 'ulid'
import { exchangeCodeAndIdentify } from '$lib/server/oauth/connectorOAuth'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { logger } from '$lib/server/logger'
import { getSourcesByType } from '$lib/server/db/sources'

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
    if (state.metadata.strictScopeCheck && flow.type === 'user_write') {
        const missing = requiredScopes.filter((s) => !grantedScopes.includes(s))
        if (missing.length > 0) {
            const params = new URLSearchParams({
                ok: 'false',
                sourceId: flow.sourceId,
                message: `Missing required scopes: ${missing.join(', ')}`,
            })
            throw redirect(302, `/oauth/done?${params}`)
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
        const params = new URLSearchParams({ ok: 'true', sourceId: flow.sourceId })
        throw redirect(302, `/oauth/done?${params}`)
    }

    // connect_source flow: for each requested source_type, attach to existing
    // org source if there is one; otherwise create a personal source.
    for (const sourceType of flow.sourceTypes) {
        const sourcesOfType = await getSourcesByType(sourceType)
        const orgSource = sourcesOfType.find((s) => s.scope === 'org')

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

        const existing = sourcesOfType.find((s) => s.createdBy === locals.user.id)

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
