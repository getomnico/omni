import { json, error } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getConfigValue, setConfigValue } from '$lib/server/db/configuration'

const VALID_MODES = new Set(['off', 'chat', 'full'])

export const GET: RequestHandler = async ({ locals }) => {
    if (!locals.user || locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const config = await getConfigValue('memory_mode_default')
    const mode = (config?.mode as string) ?? 'off'
    return json({ mode })
}

export const PATCH: RequestHandler = async ({ request, locals }) => {
    if (!locals.user || locals.user.role !== 'admin') {
        throw error(403, 'Admin access required')
    }

    const body = await request.json().catch(() => null)
    if (!body || typeof body !== 'object') {
        throw error(400, 'Invalid JSON body')
    }

    const { mode } = body as { mode?: unknown }
    if (typeof mode !== 'string' || !VALID_MODES.has(mode)) {
        throw error(400, 'mode must be one of: off, chat, full')
    }

    await setConfigValue('memory_mode_default', { mode })
    return json({ ok: true, mode })
}
