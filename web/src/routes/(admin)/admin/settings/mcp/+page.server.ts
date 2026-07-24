import { requireAdmin } from '$lib/server/authHelpers'
import type { PageServerLoad } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)
    return {}
}
