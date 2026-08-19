import { redirect, fail } from '@sveltejs/kit'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import { db } from '$lib/server/db'
import { sources, documents } from '$lib/server/db/schema'
import { eq, and, count, inArray } from 'drizzle-orm'
import { updateSourceById } from '$lib/server/db/sources'
import { sourcesRepository } from '$lib/server/repositories/sources'
import {
    getOAuthManifestForSourceType,
    oauthServiceBaseUrl,
} from '$lib/server/oauth/connectorOAuth'
import { SourceType, supportsDataSync } from '$lib/types'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    const googleConnectorConfig = await getConnectorConfigPublic('google')

    // The Windshift server URL is an admin setting stored in connector_configs.
    // Fall back to the connector manifest (env-var based deployments) until the
    // admin configures it in the UI.
    const windshiftConnectorConfig = await getConnectorConfigPublic('windshift')
    const storedWindshiftBaseUrl = windshiftConnectorConfig?.config?.base_url
    let windshiftBaseUrl: string | null =
        typeof storedWindshiftBaseUrl === 'string' && storedWindshiftBaseUrl.trim().length > 0
            ? storedWindshiftBaseUrl
            : null
    if (!windshiftBaseUrl) {
        try {
            const oauth = await getOAuthManifestForSourceType(SourceType.WINDSHIFT)
            if (oauth?.auth_endpoint) windshiftBaseUrl = oauthServiceBaseUrl(oauth.auth_endpoint)
        } catch (err) {
            locals.logger.warn('Failed to load the Windshift connector URL', err)
        }
    }

    const userSources = (await sourcesRepository.getByUserId(locals.user.id)).filter((source) =>
        supportsDataSync(source.integrationType),
    )

    // Load sync status and document counts for personal syncable sources owned by this user
    const userSourceIds = userSources.map((s) => s.id)
    const latestSyncRuns = await sourcesRepository.getLatestSyncRunsForSourceIds(userSourceIds)

    let documentCounts: Record<string, number> = {}
    if (userSourceIds.length > 0) {
        const counts = await locals.db
            .select({
                sourceId: documents.sourceId,
                count: count(),
            })
            .from(documents)
            .where(inArray(documents.sourceId, userSourceIds))
            .groupBy(documents.sourceId)
        for (const row of counts) {
            documentCounts[row.sourceId] = row.count
        }
    }

    return {
        googleOAuthConfigured: !!(
            googleConnectorConfig && googleConnectorConfig.config.oauth_client_id
        ),
        windshiftBaseUrl,
        userSources,
        latestSyncRuns,
        documentCounts,
    }
}

export const actions: Actions = {
    disable: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const sourceId = formData.get('sourceId') as string
        if (!sourceId) {
            return fail(400, { error: 'Source ID is required' })
        }

        // Verify ownership
        const [source] = await db
            .select()
            .from(sources)
            .where(and(eq(sources.id, sourceId), eq(sources.createdBy, locals.user.id)))
            .limit(1)

        if (!source) {
            return fail(403, { error: 'Source not found or not owned by you' })
        }
        if (!supportsDataSync(source.integrationType)) {
            return fail(400, { error: 'Source does not support data sync' })
        }

        await updateSourceById(sourceId, { isActive: false })
    },

    enable: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const sourceId = formData.get('sourceId') as string
        if (!sourceId) {
            return fail(400, { error: 'Source ID is required' })
        }

        // Verify ownership
        const [source] = await db
            .select()
            .from(sources)
            .where(and(eq(sources.id, sourceId), eq(sources.createdBy, locals.user.id)))
            .limit(1)

        if (!source) {
            return fail(403, { error: 'Source not found or not owned by you' })
        }
        if (!supportsDataSync(source.integrationType)) {
            return fail(400, { error: 'Source does not support data sync' })
        }

        await updateSourceById(sourceId, { isActive: true })
    },
}
