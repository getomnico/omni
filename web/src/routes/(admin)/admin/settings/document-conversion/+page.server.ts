import { fail } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { SystemSettings } from '$lib/server/system-flags'

const VALID_PRESETS = ['fast', 'balanced', 'quality']

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const doclingEnabled = await SystemSettings.isDoclingEnabled()
    const qualityPreset = await SystemSettings.getDoclingQualityPreset()

    // Quick health check to see if the service is reachable
    let doclingReachable = false
    try {
        const controller = new AbortController()
        const timeout = setTimeout(() => controller.abort(), 2000)
        const res = await fetch(`${env.DOCLING_URL}/health`, { signal: controller.signal })
        clearTimeout(timeout)
        if (res.ok) {
            const body = await res.json()
            doclingReachable = body.status === 'ok'
        }
    } catch {
        // Service unreachable — leave doclingReachable as false
    }

    return {
        doclingEnabled,
        doclingReachable,
        qualityPreset,
    }
}

export const actions: Actions = {
    updateDocling: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const enabled = formData.has('enabled')

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

    updateQualityPreset: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const preset = formData.get('preset') as string

        if (!preset || !VALID_PRESETS.includes(preset)) {
            return fail(400, {
                error: `Invalid preset. Must be one of: ${VALID_PRESETS.join(', ')}`,
            })
        }

        try {
            await SystemSettings.setDoclingQualityPreset(preset)
            return {
                success: true,
                message: `Quality preset updated to "${preset}"`,
            }
        } catch (err) {
            console.error('Failed to update quality preset:', err)
            return fail(500, { error: 'Failed to update quality preset' })
        }
    },
}
