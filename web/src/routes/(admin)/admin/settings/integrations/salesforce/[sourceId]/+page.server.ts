import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { getOAuthManifestForSourceType } from '$lib/server/oauth/connectorOAuth'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { SourceType } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    const { user } = requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.SALESFORCE) {
        throw error(400, 'Invalid source type for this page')
    }

    const [oauthManifest, actionCredentials] = await Promise.all([
        getOAuthManifestForSourceType(SourceType.SALESFORCE),
        serviceCredentialsRepository.getByUserAndSource(source.id, user.id),
    ])
    const credentialConfig = (actionCredentials?.config ?? {}) as Record<string, unknown>
    const grantedScopes = Array.isArray(credentialConfig.granted_scopes)
        ? credentialConfig.granted_scopes.filter(
              (scope): scope is string => typeof scope === 'string',
          )
        : []

    return {
        source,
        actionAuth: {
            authorized: Boolean(actionCredentials),
            principalEmail: actionCredentials?.principalEmail ?? null,
            grantedScopes,
        },
        oauth: {
            registrationRequiresInitialAccessToken:
                oauthManifest?.registration_requires_initial_access_token === true,
        },
    }
}

export const actions: Actions = {
    default: async ({ request, params, locals }) => {
        const user = locals.user
        if (!user || user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }

        const source = await getSourceById(params.sourceId)
        if (!source) {
            throw error(404, 'Source not found')
        }

        if (source.sourceType !== SourceType.SALESFORCE) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()
        const isActive = formData.has('enabled')

        try {
            await updateSourceById(source.id, {
                isActive,
                config: source.config || {},
            })

            const config = (source.config ?? {}) as Record<string, unknown>
            if (isActive && config.sync_enabled !== false) {
                const connectorManagerUrl = getConfig().services.connectorManagerUrl
                try {
                    await fetch(`${connectorManagerUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Salesforce settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
