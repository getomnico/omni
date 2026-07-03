import { redirect } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
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
    const recentChatsLimit = 20
    const [starredChats, recentChatRows] = await Promise.all([
        chatRepository.getByUserId(locals.user.id, { isStarred: true }),
        chatRepository.getByUserId(locals.user.id, {
            limit: recentChatsLimit + 1,
            isStarred: false,
        }),
    ])
    const recentChatsHasMore = recentChatRows.length > recentChatsLimit
    const recentChats = recentChatsHasMore
        ? recentChatRows.slice(0, recentChatsLimit)
        : recentChatRows

    return {
        user: locals.user,
        starredChats,
        recentChats,
        recentChatsHasMore,
        agentsEnabled: env.AGENTS_ENABLED === 'true',
        memoryEnabled: env.MEMORY_ENABLED === 'true',
    }
}
