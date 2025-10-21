import { redirect } from '@sveltejs/kit'
import type { LayoutServerLoad } from './$types.js'

export const load: LayoutServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    if (!locals.user.isActive) {
        throw redirect(302, '/login?error=account-inactive')
    }

    return {
        user: locals.user,
    }
}
