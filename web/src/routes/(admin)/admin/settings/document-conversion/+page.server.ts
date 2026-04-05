import { fail, redirect } from '@sveltejs/kit'
import { env } from '$env/dynamic/private'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { SystemSettings } from '$lib/server/system-flags'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    // Redirect if docling is not configured (DOCLING_URL not set)
    if (!env.DOCLING_URL) {
        redirect(302, '/admin/settings')
    }

    const doclingEnabled = await SystemSettings.isDoclingEnabled()
    const doclingOverriddenByEnv = SystemSettings.isDoclingOverriddenByEnv()

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
        doclingOverriddenByEnv,
        doclingEnvValue: env.DOCLING_ENABLED,
        doclingReachable,
    }
}

export const actions: Actions = {
    updateDocling: async ({ request, locals }) => {
        requireAdmin(locals)

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
