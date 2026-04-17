import { redirect } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { LayoutServerLoad } from './$types.js'
import { ChatRepository } from '$lib/server/db/chats.js'

export const load: LayoutServerLoad = async ({ locals, depends }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    if (!locals.user.isActive) {
        throw redirect(302, '/login?error=account-inactive')
    }

    depends('app:recent_chats')
    const chatRepo = new ChatRepository(locals.db)
    const [starredChats, recentChats] = await Promise.all([
        chatRepo.getByUserId({ isStarred: true }),
        chatRepo.getByUserId({ limit: 20, isStarred: false }),
    ])

    return {
        user: locals.user,
        starredChats,
        recentChats,
        agentsEnabled: env.AGENTS_ENABLED === 'true',
    }
}
