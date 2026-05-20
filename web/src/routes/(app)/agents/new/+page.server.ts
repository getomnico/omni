import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { getConfig } from '$lib/server/config.js'
import { listAllActiveModels } from '$lib/server/db/model-providers.js'

export const load: PageServerLoad = async ({ locals }) => {
    const { user } = requireActiveUser(locals)

    let sources: any[] = []
    try {
        const config = getConfig()
        const resp = await fetch(`${config.services.connectorManagerUrl}/sources`)
        if (resp.ok) {
            const overviews = await resp.json()
            const allSources = overviews.map((overview: any) => overview.source ?? overview)
            sources = allSources.filter((s: any) => s.is_active && !s.is_deleted)
        } else {
            locals.logger.error('Failed to fetch sources', undefined, { status: resp.status })
        }
    } catch (error) {
        locals.logger.error('Failed to fetch sources from connector manager', error)
    }

    const models = await listAllActiveModels()

    return { user, sources, models }
}
