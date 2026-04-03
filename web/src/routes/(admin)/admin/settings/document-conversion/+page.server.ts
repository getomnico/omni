import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { SystemSettings } from '$lib/server/system-flags'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const doclingEnabled = await SystemSettings.isDoclingEnabled()
    const doclingOverriddenByEnv = SystemSettings.isDoclingOverriddenByEnv()

    return {
        doclingEnabled,
        doclingOverriddenByEnv,
        doclingEnvValue: process.env.DOCLING_ENABLED,
    }
}

export const actions: Actions = {
    updateDocling: async ({ request, locals }) => {
        requireAdmin(locals)

        // Check if setting is overridden by environment variable
        if (SystemSettings.isDoclingOverriddenByEnv()) {
            return fail(400, {
                error: 'Docling setting is controlled by DOCLING_ENABLED environment variable',
            })
        }

        const formData = await request.formData()
        const enabled = formData.get('enabled') === 'true'

        try {
            await SystemSettings.setDoclingEnabled(enabled)
            return {
                success: true,
                message: enabled
                    ? 'Docling document conversion enabled'
                    : 'Docling document conversion disabled',
            }
        } catch (err) {
            console.error('Failed to update Docling setting:', err)
            return fail(500, { error: 'Failed to update setting' })
        }
    },
}
