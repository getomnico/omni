import { redirect } from '@sveltejs/kit'
import type { LayoutServerLoad } from './$types.js'
import { chatRepository } from '$lib/server/db/chats.js'

export const load: LayoutServerLoad = async ({ locals, depends }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    if (!locals.user.isActive) {
        throw redirect(302, '/login?error=account-inactive')
    }

    depends('app:recent_chats')
    const recentChats = await chatRepository.getByUserId(locals.user.id, 20, 0)
    return {
        user: locals.user,
        recentChats,
    }
}
