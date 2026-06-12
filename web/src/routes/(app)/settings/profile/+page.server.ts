import { env } from '$env/dynamic/private'
import { fail, redirect } from '@sveltejs/kit'
import type { Actions, PageServerLoad } from './$types'
import { listAllActiveModels } from '$lib/server/db/model-providers'
import { userRepository } from '$lib/server/db/users'
import {
    DEFAULT_TIMEZONE,
    normalizeTimezone,
    setUserTimezone,
} from '$lib/server/db/userConfiguration.js'

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    const [allModels, dbUser] = await Promise.all([
        listAllActiveModels(),
        userRepository.findById(locals.user.id),
    ])

    return {
        user: locals.user,
        timezone: locals.user.configuration.timezone ?? DEFAULT_TIMEZONE,
        timezoneSaved: locals.user.configuration.timezone !== null,
        canChangePassword: !!dbUser?.passwordHash,
        memoryEnabled: env.MEMORY_ENABLED === 'true',
        models: allModels.map((model) => ({
            id: model.id,
            displayName: model.displayName,
            providerType: model.providerType,
            isDefault: model.isDefault,
        })),
    }
}

export const actions: Actions = {
    saveTimezone: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const rawTimezone = formData.get('timezone')
        if (typeof rawTimezone !== 'string') {
            return fail(400, { error: 'Timezone is required' })
        }

        const timezone = normalizeTimezone(rawTimezone)
        if (!timezone) {
            return fail(400, { error: 'Choose a valid timezone' })
        }

        try {
            const savedTimezone = await setUserTimezone(locals.user.id, timezone)
            locals.user.configuration.timezone = savedTimezone
            return { success: true, timezone: savedTimezone }
        } catch (error) {
            locals.logger.error('Failed to save timezone setting', error as Error, {
                userId: locals.user.id,
            })
            return fail(500, { error: 'Failed to save timezone' })
        }
    },
}
