import { redirect } from '@sveltejs/kit'
import { deleteSessionTokenCookie, invalidateSession } from '$lib/server/auth.js'
import type { RequestHandler } from './$types.js'

export const POST: RequestHandler = async ({ cookies, locals }) => {
    if (locals.session) {
        await invalidateSession(locals.session.id)
    }

    deleteSessionTokenCookie(cookies)
    throw redirect(302, '/login')
}
