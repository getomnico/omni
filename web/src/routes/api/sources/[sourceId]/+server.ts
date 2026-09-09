import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { db } from '$lib/server/db'
import { sources, syncRuns, serviceCredentials } from '$lib/server/db/schema'
import { eq, and } from 'drizzle-orm'
import { getConfig } from '$lib/server/config'
import {
    clientConfigProviderForSource,
    getOAuthConfigForSource,
    getOAuthManifestForSourceType,
    isAutoManagedOAuthProvider,
    removeDynamicallyRegisteredClient,
} from '$lib/server/oauth/connectorOAuth'
import { logger } from '$lib/server/logger'
import {
    isValidSyncIntervalSeconds,
    MAX_SYNC_INTERVAL_SECONDS,
    MIN_SYNC_INTERVAL_SECONDS,
} from '$lib/utils/sync-interval'

export const PATCH: RequestHandler = async ({ params, request, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    if (locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const sourceId = params.sourceId

    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    let body: unknown
    try {
        body = await request.json()
    } catch {
        throw error(400, 'Invalid JSON body')
    }

    const syncIntervalSeconds =
        body && typeof body === 'object' && 'syncIntervalSeconds' in body
            ? (body as { syncIntervalSeconds: unknown }).syncIntervalSeconds
            : undefined

    if (!isValidSyncIntervalSeconds(syncIntervalSeconds)) {
        throw error(
            400,
            `syncIntervalSeconds must be a positive integer between ${MIN_SYNC_INTERVAL_SECONDS} and ${MAX_SYNC_INTERVAL_SECONDS}`,
        )
    }

    const [updatedSource] = await db
        .update(sources)
        .set({
            syncIntervalSeconds,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))
        .returning({ syncIntervalSeconds: sources.syncIntervalSeconds })

    return json({ syncIntervalSeconds: updatedSource.syncIntervalSeconds })
}

export const DELETE: RequestHandler = async ({ params, locals, fetch, url }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const sourceId = params.sourceId

    const source = await db.query.sources.findFirst({
        where: eq(sources.id, sourceId),
    })

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (locals.user.role !== 'admin' && source.createdBy !== locals.user.id) {
        throw error(403, 'Admin access required')
    }

    const config = getConfig()
    const connectorManagerUrl = config.services.connectorManagerUrl

    // Cancel any running sync for this source
    const runningSyncs = await db.query.syncRuns.findMany({
        where: and(eq(syncRuns.sourceId, sourceId), eq(syncRuns.status, 'running')),
    })

    for (const sync of runningSyncs) {
        try {
            await fetch(`${connectorManagerUrl}/sync/${sync.id}/cancel`, {
                method: 'POST',
            })
        } catch (err) {
            logger.warn(`Failed to cancel sync ${sync.id} for source ${sourceId}`, err)
        }
    }

    // Revoke a source-scoped dynamically registered OAuth client before
    // removing the source, then remove its encrypted local metadata. The
    // connector manifest owns the source-key template; this route remains
    // provider-agnostic.
    const revokeOAuthClient = url.searchParams.get('revoke_oauth_client') !== 'false'
    const oauthManifest =
        (await getOAuthConfigForSource(source)) ??
        (await getOAuthManifestForSourceType(source.sourceType))
    const sourceOAuthProvider =
        oauthManifest &&
        oauthManifest.client_config_provider_template &&
        isAutoManagedOAuthProvider(oauthManifest)
            ? clientConfigProviderForSource(oauthManifest, sourceId)
            : null
    if (
        sourceOAuthProvider &&
        sourceOAuthProvider !== oauthManifest?.provider &&
        revokeOAuthClient
    ) {
        try {
            await removeDynamicallyRegisteredClient(sourceOAuthProvider)
        } catch (err) {
            logger.warn(`Failed to revoke the source OAuth client for ${sourceId}`, err)
            throw error(
                502,
                'The OAuth provider is unavailable to revoke its client; try deleting the source again, or delete with ?revoke_oauth_client=false to keep the registered client',
            )
        }
    }

    // Delete service credentials eagerly (small table, contains sensitive OAuth tokens)
    await db.delete(serviceCredentials).where(eq(serviceCredentials.sourceId, sourceId))

    // Soft-delete the source — background cleanup in connector-manager will handle documents/embeddings
    await db
        .update(sources)
        .set({
            isActive: false,
            isDeleted: true,
            updatedAt: new Date(),
        })
        .where(eq(sources.id, sourceId))

    return json({ success: true })
}
