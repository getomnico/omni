import { json } from '@sveltejs/kit'
import { z } from 'zod'
import type { RequestHandler } from './$types'
import { normalizeTimezone, setUserTimezone } from '$lib/server/db/userConfiguration.js'
import type { UserConfiguration } from '$lib/types/userConfiguration'

const userSettingsUpdateSchema = z
    .object({
        timezone: z.string().optional(),
    })
    .strict()

type UserSettingsUpdate = z.infer<typeof userSettingsUpdateSchema>
type UserSettingsResponse = { settings: Partial<UserConfiguration> } | { error: string }

export const POST: RequestHandler = async ({ request, locals }) => {
    if (!locals.user) {
        return json({ error: 'Unauthorized' } satisfies UserSettingsResponse, { status: 401 })
    }

    let update: UserSettingsUpdate
    try {
        update = userSettingsUpdateSchema.parse(await request.json())
    } catch {
        return json({ error: 'Invalid user settings payload' } satisfies UserSettingsResponse, {
            status: 400,
        })
    }

    const savedSettings: Partial<UserConfiguration> = {}

    if (update.timezone !== undefined) {
        const normalized = normalizeTimezone(update.timezone)
        if (!normalized) {
            return json({ error: 'Invalid timezone' } satisfies UserSettingsResponse, {
                status: 400,
            })
        }

        try {
            const savedTimezone = await setUserTimezone(locals.user.id, normalized)
            locals.user.configuration.timezone = savedTimezone
            savedSettings.timezone = savedTimezone
        } catch (error) {
            locals.logger.error('Failed to save user timezone setting', error as Error, {
                userId: locals.user.id,
            })
            return json({ error: 'Failed to save user settings' } satisfies UserSettingsResponse, {
                status: 500,
            })
        }
    }

    if (Object.keys(savedSettings).length === 0) {
        return json(
            { error: 'No supported user settings provided' } satisfies UserSettingsResponse,
            {
                status: 400,
            },
        )
    }

    return json({ settings: savedSettings } satisfies UserSettingsResponse)
}
