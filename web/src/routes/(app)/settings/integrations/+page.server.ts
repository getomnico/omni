import { redirect, fail } from '@sveltejs/kit'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import { SourcesRepository } from '$lib/server/repositories/sources'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    if (locals.user.role === 'admin') {
        throw redirect(302, '/admin/settings/integrations')
    }

    const googleConnectorConfig = await getConnectorConfigPublic('google')

    const repo = new SourcesRepository(locals.db)
    const userSources = await repo.getByUserId(locals.user.id)
    const orgWideSources = await repo.getOrgWide()

    return {
        googleOAuthConfigured: !!(
            googleConnectorConfig && googleConnectorConfig.config.oauth_client_id
        ),
        orgWideSources,
        userSources,
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

        const repo = new SourcesRepository(locals.db)
        const source = await repo.getById(sourceId)

        if (!source || source.createdBy !== locals.user.id) {
            return fail(403, { error: 'Source not found or not owned by you' })
        }

        await repo.updateById(sourceId, { isActive: false })
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

        const repo = new SourcesRepository(locals.db)
        const source = await repo.getById(sourceId)

        if (!source || source.createdBy !== locals.user.id) {
            return fail(403, { error: 'Source not found or not owned by you' })
        }

        await repo.updateById(sourceId, { isActive: true })
    },
}
