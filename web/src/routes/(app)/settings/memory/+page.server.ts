import { redirect, fail } from '@sveltejs/kit'
import { getConfigValue } from '$lib/server/db/configuration'
import { userRepository } from '$lib/server/db/users'
import type { PageServerLoad, Actions } from './$types'

const VALID_MODES = ['off', 'chat', 'full', '']

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    const orgDefaultConfig = await getConfigValue('memory_mode_default')
    const orgDefault = (orgDefaultConfig?.value as string) ?? 'off'

    return {
        currentMode: locals.user.memoryMode ?? null,
        orgDefault,
    }
}

export const actions: Actions = {
    default: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const mode = formData.get('mode') as string

        if (!VALID_MODES.includes(mode)) {
            return fail(400, { error: 'Invalid memory mode' })
        }

        try {
            await userRepository.update(locals.user.id, {
                memoryMode: mode === '' ? null : mode,
            })
            return { success: true }
        } catch (err) {
            console.error('Failed to update memory mode:', err)
            return fail(500, { error: 'Failed to save preference' })
        }
    },
}
