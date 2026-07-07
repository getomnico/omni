import { error, redirect } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { ulid } from 'ulid'
import { exchangeCodeAndIdentify } from '$lib/server/oauth/connectorOAuth'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { decryptConfig } from '$lib/server/crypto/encryption'
import { logger } from '$lib/server/logger'
import { getConfig } from '$lib/server/config'
import { getSourceById, getSourcesByType } from '$lib/server/db/sources'
import { getSourceDisplayName } from '$lib/utils/icons'
import { SourceType } from '$lib/types'

/// Unified OAuth callback. Provider-agnostic — dispatches based on the flow
/// stored in the OAuth state.
export const GET: RequestHandler = async ({ url, locals, fetch }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }
    const user = locals.user

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

    const { tokens, state, principalEmail, config, clientCreds } = exchange

    if (state.user_id !== user.id) {
        throw error(403, 'OAuth state does not match the signed-in user')
    }
    if (!state.metadata) {
        throw error(400, 'OAuth state has no metadata')
    }

    const flow = state.metadata.flow
    const grantedScopes = (tokens.scope ?? '')
        .split(config.scope_separator === ',' ? ',' : /[\s,]+/)
        .filter(Boolean)
    const requiredScopes = state.metadata.requiredScopes
    // OAuth token responses may omit `scope` when the granted scopes match the
    // request. Treat omission as the requested scopes for validation/storage.
    const effectiveGrantedScopes = grantedScopes.length > 0 ? grantedScopes : requiredScopes
    if (state.metadata.strictScopeCheck && flow.type === 'user_write') {
        const missing = requiredScopes.filter((s) => !effectiveGrantedScopes.includes(s))
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
    const credentialsWithRefreshFallback = (existingCredentials: Record<string, unknown>) => ({
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token ?? existingCredentials.refresh_token ?? null,
        token_type: tokens.token_type ?? 'Bearer',
        client_id: clientCreds.clientId,
        ...(clientCreds.clientSecret ? { client_secret: clientCreds.clientSecret } : {}),
        token_uri: clientCreds.tokenEndpoint ?? config.token_endpoint,
    })

    if (flow.type === 'org_source') {
        if (user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }
        const source = await getSourceById(flow.sourceId)
        if (!source || source.isDeleted || source.scope !== 'org') {
            throw error(404, 'Org source not found')
        }
        const existing = await serviceCredentialsRepository.getOrgCredsBySourceId(flow.sourceId)
        const existingCredentials = existing ? decryptConfig(existing.credentials) : {}

        await serviceCredentialsRepository.create({
            sourceId: flow.sourceId,
            provider: config.provider,
            authType: 'oauth',
            principalEmail,
            credentials: {
                ...existingCredentials,
                ...credentialsWithRefreshFallback(existingCredentials),
            },
            config: (existing?.config as Record<string, unknown> | undefined) ?? {},
            expiresAt,
        })

        await notifyOAuthCredentialReady(flow.sourceId)

        try {
            await fetch(`/api/sources/${flow.sourceId}/sync`, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ sync_mode: 'full' }),
            })
        } catch (syncError) {
            logger.warn('Failed to trigger post-OAuth sync', { sourceId: flow.sourceId, syncError })
        }

        throw redirect(302, flow.returnTo ?? '/admin/settings/integrations?success=connected')
    }

    const notifyOAuthCredentialReady = async (sourceId: string, userId?: string) => {
        try {
            const cmUrl = getConfig().services.connectorManagerUrl
            const resp = await fetch(`${cmUrl}/oauth/credential-ready`, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    source_id: sourceId,
                    user_id: userId ?? null,
                    provider: config.provider,
                    flow: flow.type === 'user_write' ? 'user_write' : 'user_read',
                }),
            })
            if (!resp.ok) {
                logger.warn('OAuth credential-ready notification failed', {
                    sourceId,
                    status: resp.status,
                    body: await resp.text(),
                })
            }
        } catch (err) {
            logger.warn('OAuth credential-ready notification failed', {
                sourceId,
                err: String(err),
            })
        }
    }

    if (flow.type === 'user_read' || flow.type === 'user_write') {
        const existing = await serviceCredentialsRepository.getByUserAndSource(
            flow.sourceId,
            user.id,
        )
        const existingCredentials = existing ? decryptConfig(existing.credentials) : {}

        await serviceCredentialsRepository.createForUser({
            sourceId: flow.sourceId,
            userId: user.id,
            provider: config.provider,
            authType: 'oauth',
            principalEmail,
            credentials: {
                ...existingCredentials,
                ...credentialsWithRefreshFallback(existingCredentials),
            },
            config: { granted_scopes: effectiveGrantedScopes },
            expiresAt,
        })
        await notifyOAuthCredentialReady(flow.sourceId, user.id)
        if (flow.returnTo) {
            throw redirect(302, flow.returnTo)
        }
        const params = new URLSearchParams({ ok: 'true', sourceId: flow.sourceId })
        throw redirect(302, `/oauth/done?${params}`)
    }

    // connect_source flow: for each requested source_type, create or refresh this
    // user's personal source. Org-level sources are managed separately under
    // /admin/settings/integrations.
    for (const sourceType of flow.sourceTypes) {
        const sourcesOfType = await getSourcesByType(sourceType)
        const existing = sourcesOfType.find((s) => s.scope === 'user' && s.createdBy === user.id)

        if (existing) {
            // Source already exists for this user — refresh its creds in place.
            const existingCredential = await serviceCredentialsRepository.getByUserAndSource(
                existing.id,
                user.id,
            )
            const existingCredentials = existingCredential
                ? decryptConfig(existingCredential.credentials)
                : {}
            await serviceCredentialsRepository.createForUser({
                sourceId: existing.id,
                userId: user.id,
                provider: config.provider,
                authType: 'oauth',
                principalEmail,
                credentials: {
                    ...existingCredentials,
                    ...credentialsWithRefreshFallback(existingCredentials),
                },
                config: { granted_scopes: effectiveGrantedScopes },
                expiresAt,
            })
            continue
        }

        const [newSource] = await db
            .insert(sources)
            .values({
                id: ulid(),
                name: getSourceDisplayName(sourceType as SourceType) ?? sourceType,
                sourceType,
                scope: 'user',
                config: {},
                createdBy: user.id,
                isActive: true,
            })
            .returning()

        await serviceCredentialsRepository.createForUser({
            sourceId: newSource.id,
            userId: user.id,
            provider: config.provider,
            authType: 'oauth',
            principalEmail,
            credentials: credentialsWithRefreshFallback({}),
            config: { granted_scopes: effectiveGrantedScopes },
            expiresAt,
        })

        logger.info(`Created personal source ${newSource.id} (${sourceType}) for user ${user.id}`)
    }

    throw redirect(302, flow.returnTo ?? '/settings/integrations?success=connected')
}
