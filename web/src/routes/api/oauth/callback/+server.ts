import { error, redirect } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { serviceCredentials, sources } from '$lib/server/db/schema'
import { ulid } from 'ulid'
import { and, eq, isNull } from 'drizzle-orm'
import {
    exchangeCodeAndIdentify,
    oauthCredentialExpiry,
    requestOAuthCredentialValidation,
    SOURCE_BINDING_CONFIG_KEY,
    type OAuthSourceBinding,
} from '$lib/server/oauth/connectorOAuth'
import { OAuthStateManager } from '$lib/server/oauth/state'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { decryptConfig, encryptConfig } from '$lib/server/crypto/encryption'
import { logger } from '$lib/server/logger'
import { getConfig } from '$lib/server/config'
import { getSourceById, getSourcesByType } from '$lib/server/db/sources'
import { toolApprovalRepository } from '$lib/server/db/tool-approvals'
import { getSourceDisplayName } from '$lib/utils/icons'
import { IntegrationType, SourceType } from '$lib/types'

function isSafeLocalPath(value: string): boolean {
    return value.startsWith('/') && !value.startsWith('//')
}

function withErrorParam(path: string, errorCode: string): string {
    const separator = path.includes('?') ? '&' : '?'
    return `${path}${separator}error=${encodeURIComponent(errorCode)}`
}

function returnToFromStateMetadata(metadata: Record<string, unknown> | undefined): string | null {
    const flow = metadata?.flow
    if (!flow || typeof flow !== 'object' || Array.isArray(flow)) {
        return null
    }
    const returnTo = (flow as { returnTo?: unknown }).returnTo
    return typeof returnTo === 'string' && isSafeLocalPath(returnTo) ? returnTo : null
}

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
        let returnTo: string | null = null
        if (stateToken) {
            try {
                const pendingState = await OAuthStateManager.getState(stateToken)
                if (pendingState?.user_id === user.id) {
                    returnTo = returnToFromStateMetadata(pendingState.metadata)
                }
            } catch (err) {
                logger.warn('Failed to read OAuth state after provider denial', {
                    err: String(err),
                })
            }
        }
        throw redirect(302, withErrorParam(returnTo ?? '/settings/integrations', 'oauth_denied'))
    }
    if (!code || !stateToken) {
        throw error(400, 'Missing code or state')
    }

    let failureReturnTo: string | null = null
    try {
        const pendingState = await OAuthStateManager.getState(stateToken)
        if (pendingState?.user_id === user.id) {
            failureReturnTo = returnToFromStateMetadata(pendingState.metadata)
        }
    } catch (err) {
        logger.warn('Failed to read OAuth state for failure redirect', { err: String(err) })
    }

    let exchange
    try {
        exchange = await exchangeCodeAndIdentify(code, stateToken, {
            authenticatedUserEmail: user.email,
        })
    } catch (err) {
        logger.error('OAuth exchange failed', { err: String(err) })
        throw redirect(
            302,
            withErrorParam(failureReturnTo ?? '/settings/integrations', 'oauth_failed'),
        )
    }

    const { tokens, state, principalEmail, config, clientCreds, userinfo } = exchange
    const credentialProvider = config.credential_provider ?? config.provider

    if (state.user_id !== user.id) {
        throw error(403, 'OAuth state does not match the signed-in user')
    }
    if (!state.metadata) {
        throw error(400, 'OAuth state has no metadata')
    }

    const flow = state.metadata.flow
    const redirectOAuthFailure = (message: string): never => {
        logger.error('OAuth callback validation failed', {
            provider: config.provider,
            sourceId: 'sourceId' in flow ? flow.sourceId : undefined,
            flowType: flow.type,
            message,
        })
        if (flow.type === 'org_source') {
            throw redirect(
                302,
                withErrorParam(
                    flow.returnTo ?? '/admin/settings/integrations',
                    'oauth_validation_failed',
                ),
            )
        }
        if (flow.type === 'user_read' || flow.type === 'user_write') {
            const params = new URLSearchParams({
                ok: 'false',
                sourceId: flow.sourceId,
                message,
            })
            throw redirect(302, `/oauth/done?${params}`)
        }
        throw redirect(
            302,
            withErrorParam(flow.returnTo ?? '/settings/integrations', 'oauth_failed'),
        )
    }
    const grantedScopes = (tokens.scope ?? '')
        .split(config.scope_separator === ',' ? ',' : /[\s,]+/)
        .filter(Boolean)
    const requiredScopes = state.metadata.requiredScopes
    // OAuth token responses may omit `scope` when the granted scopes match the
    // request. Treat omission as the requested scopes for validation/storage.
    const effectiveGrantedScopes = grantedScopes.length > 0 ? grantedScopes : requiredScopes
    const storedGrantedScopes = flow.type === 'user_read' ? requiredScopes : effectiveGrantedScopes
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

    const tokenResponseMetadata = Object.fromEntries(
        (config.token_response_fields ?? [])
            .filter((field) => !['access_token', 'refresh_token', 'token_type'].includes(field))
            .flatMap((field) => {
                const value = tokens[field]
                return value === undefined ? [] : [[field, value]]
            }),
    )
    const credentialsWithRefreshFallback = (existingCredentials: Record<string, unknown>) => ({
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token ?? existingCredentials.refresh_token ?? null,
        token_type: tokens.token_type ?? 'Bearer',
        ...tokenResponseMetadata,
        client_id: clientCreds.clientId,
        ...(clientCreds.clientSecret ? { client_secret: clientCreds.clientSecret } : {}),
        token_uri: clientCreds.tokenEndpoint ?? config.token_endpoint,
        token_endpoint_auth_method: clientCreds.tokenEndpointAuthMethod,
        ...(config.resource ? { resource: config.resource } : {}),
    })

    // Salesforce and some other providers omit `expires_in`; without an expiry
    // the stored credential is never refreshed (the manager only refreshes
    // rows whose expires_at has arrived) and the access token dies silently.
    // The default lifetime applies only when the credential carries a refresh
    // token; otherwise no expiry is persisted rather than a fabricated one.
    const credentialExpiryFor = (existingCredentials: Record<string, unknown>) =>
        oauthCredentialExpiry(tokens, tokens.refresh_token ?? existingCredentials.refresh_token)

    const validateOAuthCredentialForSource = async (
        sourceId: string,
        credentials: Record<string, unknown>,
    ): Promise<OAuthSourceBinding | null> =>
        requestOAuthCredentialValidation({
            sourceId,
            provider: config.provider,
            credentials,
            flow: flow.type,
            metadata:
                typeof userinfo === 'object' && userinfo !== null && !Array.isArray(userinfo)
                    ? (userinfo as Record<string, unknown>)
                    : {},
        })

    const notifyOAuthCredentialReady = async (
        sourceId: string,
        userId?: string,
    ): Promise<Record<string, unknown> | null> => {
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
            const body = (await resp.json().catch(() => null)) as Record<string, unknown> | null
            if (!resp.ok) {
                logger.warn('OAuth credential-ready notification failed', {
                    sourceId,
                    status: resp.status,
                    body,
                })
                return null
            }
            return body
        } catch (err) {
            logger.warn('OAuth credential-ready notification failed', {
                sourceId,
                err: String(err),
            })
            return null
        }
    }

    if (flow.type === 'org_source') {
        if (user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }
        const source = await getSourceById(flow.sourceId)
        if (!source || source.isDeleted || source.scope !== 'org') {
            return redirectOAuthFailure('Org source not found')
        }
        const existing = await serviceCredentialsRepository.getOrgCredsBySourceId(flow.sourceId)
        const existingCredentials = existing ? decryptConfig(existing.credentials) : {}
        const credentials = credentialsWithRefreshFallback(existingCredentials)
        let binding: OAuthSourceBinding | null
        try {
            binding = await validateOAuthCredentialForSource(flow.sourceId, credentials)
        } catch (err) {
            return redirectOAuthFailure(
                err instanceof Error ? err.message : 'OAuth credential rejected',
            )
        }

        // The binding and the credential must land together: a source that
        // carries the binding without its credential (or vice versa) would
        // fail validation on the next connect.
        await db.transaction(async (tx) => {
            if (binding) {
                await tx
                    .update(sources)
                    .set({
                        config: {
                            ...((source.config ?? {}) as Record<string, unknown>),
                            [SOURCE_BINDING_CONFIG_KEY]: binding,
                        },
                        updatedAt: new Date(),
                    })
                    .where(eq(sources.id, source.id))
            }
            await tx
                .delete(serviceCredentials)
                .where(
                    and(
                        eq(serviceCredentials.sourceId, flow.sourceId),
                        isNull(serviceCredentials.userId),
                    ),
                )
            await tx.insert(serviceCredentials).values({
                id: ulid(),
                sourceId: flow.sourceId,
                userId: null,
                provider: credentialProvider,
                authType: 'oauth',
                principalEmail,
                credentials: encryptConfig(credentials),
                config: (existing?.config as Record<string, unknown> | undefined) ?? {},
                expiresAt: credentialExpiryFor(existingCredentials),
            })
        })

        const credentialReady = await notifyOAuthCredentialReady(flow.sourceId)

        if (source.integrationType === IntegrationType.REMOTE_MCP) {
            if (credentialReady?.status !== 'completed') {
                throw redirect(
                    302,
                    withErrorParam(
                        flow.returnTo ?? '/admin/settings/integrations',
                        'oauth_catalog_refresh_failed',
                    ),
                )
            }
            await db
                .update(sources)
                .set({ isActive: true, updatedAt: new Date() })
                .where(eq(sources.id, flow.sourceId))
            throw redirect(302, flow.returnTo ?? '/admin/settings/integrations?success=connected')
        }

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

    if (flow.type === 'user_read' || flow.type === 'user_write') {
        const source = await getSourceById(flow.sourceId)
        if (!source || source.isDeleted) return redirectOAuthFailure('Source not found')
        const existing = await serviceCredentialsRepository.getByUserAndSource(
            flow.sourceId,
            user.id,
        )
        const existingCredentials = existing ? decryptConfig(existing.credentials) : {}
        const credentials = credentialsWithRefreshFallback(existingCredentials)
        // Validation runs for user flows too, but a returned binding is not
        // applied: the source-level binding belongs to the org connect flows.
        try {
            await validateOAuthCredentialForSource(flow.sourceId, credentials)
        } catch (err) {
            return redirectOAuthFailure(
                err instanceof Error ? err.message : 'OAuth credential rejected',
            )
        }

        await serviceCredentialsRepository.createForUser({
            sourceId: flow.sourceId,
            userId: user.id,
            provider: credentialProvider,
            authType: 'oauth',
            principalEmail,
            credentials,
            config: { granted_scopes: storedGrantedScopes },
            expiresAt: credentialExpiryFor(existingCredentials),
        })
        if (flow.type === 'user_write' && flow.approvalId) {
            if (!flow.approvalChatId || !flow.sourceType) {
                throw error(400, 'OAuth approval state is incomplete')
            }
            const approval = await toolApprovalRepository.approvePendingOAuth(
                flow.approvalId,
                user.id,
                flow.approvalChatId,
                flow.sourceId,
                flow.sourceType,
                config.provider,
            )
            if (!approval) throw error(400, 'OAuth approval is no longer pending')
        }
        await notifyOAuthCredentialReady(flow.sourceId, user.id)
        if (flow.returnTo && !(flow.type === 'user_write' && flow.approvalId)) {
            throw redirect(302, flow.returnTo)
        }
        const params = new URLSearchParams({ ok: 'true', sourceId: flow.sourceId })
        if (flow.type === 'user_write' && flow.approvalId) {
            params.set('approvalId', flow.approvalId)
        }
        throw redirect(302, `/oauth/done?${params}`)
    }

    // connect_source flow: for each requested source_type, create or refresh this
    // user's personal source. Org-level sources are managed separately under
    // /admin/settings/integrations.
    const connectedSourceIds: string[] = []
    const incrementalSyncSourceIds = new Set<string>()
    for (const sourceType of flow.sourceTypes) {
        const sourcesOfType = await getSourcesByType(sourceType)
        const existing = sourcesOfType.find((s) => s.scope === 'user' && s.createdBy === user.id)

        if (existing) {
            const existingCredential = await serviceCredentialsRepository.getByUserAndSource(
                existing.id,
                user.id,
            )
            const existingCredentials = existingCredential
                ? decryptConfig(existingCredential.credentials)
                : {}
            const accountChanged =
                sourceType === SourceType.GOOGLE_DRIVE &&
                (!existingCredential ||
                    typeof principalEmail !== 'string' ||
                    principalEmail.length === 0 ||
                    typeof existingCredential.principalEmail !== 'string' ||
                    existingCredential.principalEmail.length === 0 ||
                    existingCredential.principalEmail.trim().toLowerCase() !==
                        principalEmail.trim().toLowerCase())

            if (accountChanged) {
                // A personal Drive source is account-scoped. Do not reuse its
                // documents or folder scope for a different Google identity.
                // The old generation is hidden immediately and cleaned up by
                // connector-manager; the new account starts in pending setup.
                const replacementId = ulid()
                const replacementCredentials = credentialsWithRefreshFallback({})
                await db.transaction(async (tx) => {
                    await tx.insert(sources).values({
                        id: replacementId,
                        name: existing.name,
                        sourceType: existing.sourceType,
                        integrationType: existing.integrationType,
                        config: { index_scope: 'pending', folder_path_filters: [] },
                        isActive: false,
                        isDeleted: false,
                        scope: 'user',
                        userFilterMode: existing.userFilterMode,
                        userWhitelist: existing.userWhitelist,
                        userBlacklist: existing.userBlacklist,
                        createdBy: existing.createdBy,
                        syncIntervalSeconds: existing.syncIntervalSeconds,
                    })
                    await tx.insert(serviceCredentials).values({
                        id: ulid(),
                        sourceId: replacementId,
                        userId: user.id,
                        provider: credentialProvider,
                        authType: 'oauth',
                        principalEmail,
                        credentials: encryptConfig(replacementCredentials),
                        config: { granted_scopes: effectiveGrantedScopes },
                        expiresAt: credentialExpiryFor({}),
                    })
                    await tx
                        .update(sources)
                        .set({ isActive: false, isDeleted: true, updatedAt: new Date() })
                        .where(eq(sources.id, existing.id))
                    await tx
                        .delete(serviceCredentials)
                        .where(eq(serviceCredentials.sourceId, existing.id))
                })
                connectedSourceIds.push(replacementId)
                continue
            }

            let binding: OAuthSourceBinding | null
            try {
                binding = await validateOAuthCredentialForSource(
                    existing.id,
                    credentialsWithRefreshFallback(existingCredentials),
                )
            } catch (err) {
                return redirectOAuthFailure(
                    err instanceof Error ? err.message : 'OAuth credential rejected',
                )
            }

            // Same identity — refresh its creds in place and preserve scope.
            // The binding and the credential must land together.
            await db.transaction(async (tx) => {
                if (binding) {
                    await tx
                        .update(sources)
                        .set({
                            config: {
                                ...((existing.config ?? {}) as Record<string, unknown>),
                                [SOURCE_BINDING_CONFIG_KEY]: binding,
                            },
                            updatedAt: new Date(),
                        })
                        .where(eq(sources.id, existing.id))
                }
                await tx
                    .delete(serviceCredentials)
                    .where(
                        and(
                            eq(serviceCredentials.sourceId, existing.id),
                            eq(serviceCredentials.userId, user.id),
                        ),
                    )
                await tx.insert(serviceCredentials).values({
                    id: ulid(),
                    sourceId: existing.id,
                    userId: user.id,
                    provider: credentialProvider,
                    authType: 'oauth',
                    principalEmail,
                    credentials: encryptConfig({
                        ...existingCredentials,
                        ...credentialsWithRefreshFallback(existingCredentials),
                    }),
                    config: { granted_scopes: effectiveGrantedScopes },
                    expiresAt: credentialExpiryFor(existingCredentials),
                })
            })
            connectedSourceIds.push(existing.id)
            if (sourceType === SourceType.GOOGLE_DRIVE) {
                incrementalSyncSourceIds.add(existing.id)
            }
            continue
        }

        const isGoogleDrive = sourceType === SourceType.GOOGLE_DRIVE
        const newSourceId = ulid()
        const [newSource] = await db
            .insert(sources)
            .values({
                id: newSourceId,
                name: getSourceDisplayName(sourceType as SourceType) ?? sourceType,
                sourceType,
                scope: 'user',
                config: isGoogleDrive ? { index_scope: 'pending', folder_path_filters: [] } : {},
                createdBy: user.id,
                // Keep the source inactive until the credential has passed the
                // connector's source-binding checks.
                isActive: false,
            })
            .returning()

        const newCredentials = credentialsWithRefreshFallback({})
        let binding: OAuthSourceBinding | null
        try {
            binding = await validateOAuthCredentialForSource(newSource.id, newCredentials)
        } catch (err) {
            await db.delete(sources).where(eq(sources.id, newSource.id))
            return redirectOAuthFailure(
                err instanceof Error ? err.message : 'OAuth credential rejected',
            )
        }

        // The binding and the credential must land together.
        await db.transaction(async (tx) => {
            if (binding) {
                await tx
                    .update(sources)
                    .set({
                        config: {
                            ...((newSource.config ?? {}) as Record<string, unknown>),
                            [SOURCE_BINDING_CONFIG_KEY]: binding,
                        },
                        updatedAt: new Date(),
                    })
                    .where(eq(sources.id, newSource.id))
            }
            await tx.insert(serviceCredentials).values({
                id: ulid(),
                sourceId: newSource.id,
                userId: user.id,
                provider: credentialProvider,
                authType: 'oauth',
                principalEmail,
                credentials: encryptConfig(newCredentials),
                config: { granted_scopes: effectiveGrantedScopes },
                expiresAt: credentialExpiryFor({}),
            })
        })
        if (!isGoogleDrive) {
            await db
                .update(sources)
                .set({ isActive: true, updatedAt: new Date() })
                .where(eq(sources.id, newSource.id))
        }
        connectedSourceIds.push(newSource.id)

        logger.info(`Created personal source ${newSource.id} (${sourceType}) for user ${user.id}`)
    }

    const connectorManagerUrl = getConfig().services.connectorManagerUrl
    for (const sourceId of connectedSourceIds) {
        await notifyOAuthCredentialReady(sourceId, user.id)
        const connectedSource = await getSourceById(sourceId)
        // Personal Drive is intentionally inactive until its owner chooses an
        // explicit scope in My Integrations. Other services retain the old
        // immediate-sync behavior.
        if (
            connectedSource?.sourceType === SourceType.GOOGLE_DRIVE &&
            (connectedSource.config as { index_scope?: unknown } | null)?.index_scope === 'pending'
        ) {
            continue
        }
        try {
            const syncResponse = incrementalSyncSourceIds.has(sourceId)
                ? await fetch(`${connectorManagerUrl}/sync`, {
                      method: 'POST',
                      headers: { 'content-type': 'application/json' },
                      body: JSON.stringify({ source_id: sourceId, sync_mode: 'incremental' }),
                  })
                : await fetch(`${connectorManagerUrl}/sync/${sourceId}`, {
                      method: 'POST',
                  })
            if (!syncResponse.ok && syncResponse.status !== 409) {
                logger.warn('Failed to trigger personal source sync after OAuth', {
                    sourceId,
                    status: syncResponse.status,
                    body: await syncResponse.text(),
                })
            }
        } catch (syncError) {
            logger.warn('Failed to trigger personal source sync after OAuth', {
                sourceId,
                syncError,
            })
        }
    }

    throw redirect(302, flow.returnTo ?? '/settings/integrations?success=connected')
}
