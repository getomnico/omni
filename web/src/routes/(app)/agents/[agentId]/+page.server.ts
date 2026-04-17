import type { PageServerLoad } from './$types.js'
import { requireActiveUser } from '$lib/server/authHelpers.js'
import { requireAgentAccess, listAgentRuns } from '$lib/server/db/agents.js'
import { listAllActiveModels } from '$lib/server/db/model-providers.js'

export const load: PageServerLoad = async ({ locals, params }) => {
    const { user } = requireActiveUser(locals)
    const agent = await requireAgentAccess(params.agentId, locals.db)

    const [runs, models] = await Promise.all([
        listAgentRuns(params.agentId, 50, locals.db),
        listAllActiveModels(),
    ])

    return { user, agent, runs, models }
}
