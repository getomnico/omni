import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { userRepository } from '$lib/server/db/users'

const VALID_MODES = new Set(['off', 'chat', 'full'])

export const PATCH: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        throw error(401, 'Unauthorized')
    }

    const body = await request.json().catch(() => null)
    if (!body || typeof body !== 'object') {
        throw error(400, 'Invalid JSON body')
    }

    const { mode } = body as { mode?: unknown }

    // mode=null or undefined means "clear override, defer to org default"
    if (mode !== null && mode !== undefined && (typeof mode !== 'string' || !VALID_MODES.has(mode))) {
        throw error(400, 'mode must be one of: off, chat, full, or null')
    }

    const memoryMode: string | null =
        mode === null || mode === undefined ? null : (mode as string)

    await userRepository.update(locals.user.id, { memoryMode, updatedAt: new Date() })

    return json({ ok: true, mode: memoryMode })
}
