import { env } from '$env/dynamic/private'
import type { LayoutServerLoad } from './$types.js'

export const load: LayoutServerLoad = async () => {
    return {
        agentsEnabled: env.AGENTS_ENABLED === 'true',
    }
}
