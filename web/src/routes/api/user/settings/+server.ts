import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { normalizeTimezone, setUserTimezone } from '$lib/server/db/userConfiguration.js'

type UserSettingsUpdate = {
    timezone?: string
}

function parseSettingsUpdate(body: unknown): UserSettingsUpdate | null {
    if (!body || typeof body !== 'object' || Array.isArray(body)) return null
    const record = body as Record<string, unknown>
    const settings =
        record.settings && typeof record.settings === 'object' && !Array.isArray(record.settings)
            ? (record.settings as Record<string, unknown>)
            : record

    const update: UserSettingsUpdate = {}
    if ('timezone' in settings) {
        if (typeof settings.timezone !== 'string') return null
        update.timezone = settings.timezone
    }

    return update
}

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' }, { status: 401 })
    }

    let body: unknown
    try {
        body = await request.json()
    } catch {
        return json({ error: 'Invalid JSON in request body' }, { status: 400 })
    }

    const update = parseSettingsUpdate(body)
    if (!update) {
        return json({ error: 'Invalid user settings payload' }, { status: 400 })
    }

    const savedSettings: UserSettingsUpdate = {}

    if (update.timezone !== undefined) {
        const normalized = normalizeTimezone(update.timezone)
        if (!normalized) {
            return json({ error: 'Invalid timezone' }, { status: 400 })
        }

        try {
            const savedTimezone = await setUserTimezone(locals.user.id, normalized)
            locals.user.timezone = savedTimezone
            savedSettings.timezone = savedTimezone
        } catch (error) {
            locals.logger.error('Failed to save user timezone setting', error as Error, {
                userId: locals.user.id,
            })
            return json({ error: 'Failed to save user settings' }, { status: 500 })
        }
    }

    if (Object.keys(savedSettings).length === 0) {
        return json({ error: 'No supported user settings provided' }, { status: 400 })
    }

    return json({ settings: savedSettings })
}
