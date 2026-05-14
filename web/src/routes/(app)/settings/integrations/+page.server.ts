import { redirect, fail } from '@sveltejs/kit'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import { db } from '$lib/server/db'
import { sources, documents } from '$lib/server/db/schema'
import { eq, and, count, inArray } from 'drizzle-orm'
import { updateSourceById } from '$lib/server/db/sources'
import { sourcesRepository } from '$lib/server/repositories/sources'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    const googleConnectorConfig = await getConnectorConfigPublic('google')

    const userSources = await sourcesRepository.getByUserId(locals.user.id)
    const orgWideSources = (await sourcesRepository.getOrgWide()).filter((s) => s.isActive)

    // Load sync status and document counts for user-owned sources
    const allLatestSyncRuns = await sourcesRepository.getLatestSyncRuns()
    const userSourceIds = userSources.map((s) => s.id)
    const latestSyncRuns = new Map(
        [...allLatestSyncRuns].filter(([id]) => userSourceIds.includes(id)),
    )

    let documentCounts: Record<string, number> = {}
    if (userSourceIds.length > 0) {
        const counts = await db
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
        orgWideSources,
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

        await updateSourceById(sourceId, { isActive: true })
    },
}
