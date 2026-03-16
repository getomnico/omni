import { fail } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import {
    getGoogleAuthConfig,
    getOktaAuthConfig,
    isPasswordAuthEnabled,
    updateAuthProvider,
} from '$lib/server/db/auth-providers'
import { loadOktaOAuthService } from '$lib/server/oauth/okta'
import { UserOAuthCredentialsService } from '$lib/server/oauth/userCredentials'

export const load: PageServerLoad = async ({ locals }) => {
    requireAdmin(locals)

    const google = await getGoogleAuthConfig()
    const okta = await getOktaAuthConfig()
    const oktaSsoAvailable = (await loadOktaOAuthService()) !== null
    const passwordAuthEnabled = await isPasswordAuthEnabled()

    return {
        passwordAuthEnabled,
        google: google
            ? {
                  enabled: google.enabled,
                  clientId: google.clientId,
                  hasClientSecret: !!google.clientSecret,
              }
            : { enabled: false, clientId: '', hasClientSecret: false },
        oktaSsoAvailable,
        okta: okta
            ? {
                  enabled: okta.enabled,
                  oktaDomain: okta.oktaDomain,
                  clientId: okta.clientId,
                  hasClientSecret: !!okta.clientSecret,
              }
            : { enabled: false, oktaDomain: '', clientId: '', hasClientSecret: false },
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

    updateOkta: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const enabled = formData.get('enabled') === 'true'
        const oktaDomain = (formData.get('oktaDomain') as string)?.trim() || ''
        const clientId = (formData.get('clientId') as string)?.trim() || ''
        const clientSecret = (formData.get('clientSecret') as string) || ''

        if (enabled) {
            if (!oktaDomain) {
                return fail(400, { error: 'Okta Domain is required when enabling Okta SSO' })
            }
            if (!clientId) {
                return fail(400, { error: 'Client ID is required when enabling Okta SSO' })
            }

            const existing = await getOktaAuthConfig()
            const secretToSave = clientSecret || existing?.clientSecret || ''

            if (!secretToSave) {
                return fail(400, {
                    error: 'Client Secret is required when enabling Okta SSO',
                })
            }

            await updateAuthProvider(
                'okta',
                true,
                { oktaDomain, clientId, clientSecret: secretToSave },
                locals.user!.id,
            )

            return { success: true, message: 'Okta SSO enabled' }
        }

        const existing = await getOktaAuthConfig()
        await updateAuthProvider(
            'okta',
            false,
            {
                oktaDomain: oktaDomain || existing?.oktaDomain || '',
                clientId: clientId || existing?.clientId || '',
                clientSecret: clientSecret || existing?.clientSecret || '',
            },
            locals.user!.id,
        )

        return { success: true, message: 'Okta SSO disabled' }
    },

    updatePassword: async ({ request, locals }) => {
        requireAdmin(locals)

        const formData = await request.formData()
        const enabled = formData.get('enabled') === 'true'

        if (!enabled) {
            const google = await getGoogleAuthConfig()
            const okta = await getOktaAuthConfig()
            const googleEnabled = google?.enabled ?? false
            const oktaEnabled = okta?.enabled ?? false

            if (!googleEnabled && !oktaEnabled) {
                return fail(400, {
                    error: 'Cannot disable password authentication when no other authentication method is enabled.',
                })
            }

            const oauthCredentials = await UserOAuthCredentialsService.getUserOAuthCredentials(
                locals.user!.id,
            )
            if (oauthCredentials.length === 0) {
                return fail(400, {
                    error: 'You must sign in with Google or Okta at least once before disabling password authentication, to avoid locking yourself out.',
                })
            }
        }

        await updateAuthProvider('password', enabled, {}, locals.user!.id)

        return {
            success: true,
            message: enabled
                ? 'Password authentication enabled'
                : 'Password authentication disabled',
        }
    },
}
