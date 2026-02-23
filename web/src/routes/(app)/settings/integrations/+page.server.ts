import { redirect, fail } from '@sveltejs/kit'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import { db } from '$lib/server/db'
import { sources } from '$lib/server/db/schema'
import { eq, desc, and } from 'drizzle-orm'
import { updateSourceById } from '$lib/server/db/sources'
import { sourcesRepository } from '$lib/server/repositories/sources'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    if (locals.user.role === 'admin') {
        throw redirect(302, '/admin/settings/integrations')
    }

    const googleConnectorConfig = await getConnectorConfigPublic('google')

    // Load all sources created by the current user (active + inactive)
    const userSources = await sourcesRepository.getByUserId(locals.user.id)

    // Get latest sync runs and filter to only user's sources
    const allSyncRuns = await sourcesRepository.getLatestSyncRuns()
    const userSourceIds = new Set(userSources.map((s) => s.id))
    const userSyncRuns = new Map(
        [...allSyncRuns.entries()].filter(([sourceId]) => userSourceIds.has(sourceId)),
    )

    return {
        googleOAuthConfigured: !!(
            googleConnectorConfig && googleConnectorConfig.config.oauth_client_id
        ),
        connectedSources: userSources,
        latestSyncRuns: userSyncRuns,
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
