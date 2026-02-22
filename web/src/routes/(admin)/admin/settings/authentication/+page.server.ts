import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getGoogleAuthConfig, updateAuthProvider } from '$lib/server/db/auth-providers'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const google = await getGoogleAuthConfig()

    return {
        google: google
            ? {
                  enabled: google.enabled,
                  clientId: google.clientId,
                  hasClientSecret: !!google.clientSecret,
              }
            : { enabled: false, clientId: '', hasClientSecret: false },
    }
}

export const actions: Actions = {
    update: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const enabled = formData.get('enabled') === 'true'
        const clientId = (formData.get('clientId') as string)?.trim() || ''
        const clientSecret = (formData.get('clientSecret') as string) || ''

        if (enabled) {
            if (!clientId) {
                return fail(400, { error: 'Client ID is required when enabling Google Auth' })
            }

            // If no new secret provided, preserve the existing one
            const existing = await getGoogleAuthConfig()
            const secretToSave = clientSecret || existing?.clientSecret || ''

            if (!secretToSave) {
                return fail(400, {
                    error: 'Client Secret is required when enabling Google Auth',
                })
            }

            await updateAuthProvider(
                'google',
                true,
                { clientId, clientSecret: secretToSave },
                locals.user!.id,
            )

            return { success: true, message: 'Google Auth enabled' }
        }

        // Disabling — preserve existing config but set enabled to false
        const existing = await getGoogleAuthConfig()
        await updateAuthProvider(
            'google',
            false,
            {
                clientId: clientId || existing?.clientId || '',
                clientSecret: clientSecret || existing?.clientSecret || '',
            },
            locals.user!.id,
        )

        return { success: true, message: 'Google Auth disabled' }
    },
}
