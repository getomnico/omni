import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType, type ZendeskSourceConfig } from '$lib/types'
import { db } from '$lib/server/db'
import { serviceCredentials } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.ZENDESK) {
        throw error(400, 'Invalid source type for this page')
    }

    const creds = await db.query.serviceCredentials.findFirst({
        where: eq(serviceCredentials.sourceId, params.sourceId),
    })

    return {
        source,
        principalEmail: creds?.principalEmail ?? null,
        authType: creds?.authType ?? null,
    }
}

export const actions: Actions = {
    default: async ({ request, params, locals, fetch }) => {
        const user = locals.user
        if (!user || user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }

        const source = await getSourceById(params.sourceId)
        if (!source) {
            throw error(404, 'Source not found')
        }

        if (source.sourceType !== SourceType.ZENDESK) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const syncTickets = formData.has('syncTickets')
        const syncHelpCenter = formData.has('syncHelpCenter')
        const email = (formData.get('email') as string | null)?.trim() ?? ''
        const apiToken = (formData.get('apiToken') as string | null) ?? ''
        const accessToken = (formData.get('accessToken') as string | null) ?? ''
        const credentialType = formData.get('credentialType')

        if (credentialType !== 'api_token' && credentialType !== 'access_token') {
            throw error(400, 'Unknown credential type')
        }
        const isAccessToken = credentialType === 'access_token'

        if (!syncTickets && !syncHelpCenter) {
            throw error(400, 'Select at least one thing to sync')
        }

        const creds = await db.query.serviceCredentials.findFirst({
            where: eq(serviceCredentials.sourceId, source.id),
        })
        if (!isAccessToken) {
            if (apiToken) {
                if (!email) {
                    throw error(400, 'Admin email is required when updating the API token')
                }
            } else if (email && email !== creds?.principalEmail) {
                throw error(400, 'Enter the API token to change the admin email')
            }
        }
        if (isActive && !creds && !apiToken && !accessToken) {
            throw error(
                400,
                'Enter admin email and API token, or an access token, before enabling this source',
            )
        }

        try {
            // Spread keeps unrelated keys; the subdomain is never taken from the form.
            const config: ZendeskSourceConfig = {
                ...(source.config as ZendeskSourceConfig),
                sync_tickets: syncTickets,
                sync_help_center: syncHelpCenter,
            }

            await updateSourceById(source.id, { isActive, config })

            const newCredential = isAccessToken
                ? accessToken
                    ? {
                          auth_type: 'bearer_token',
                          principal_email: null,
                          credentials: { access_token: accessToken },
                      }
                    : null
                : apiToken
                  ? {
                        auth_type: 'api_key',
                        principal_email: email,
                        credentials: { email, api_token: apiToken },
                    }
                  : null
            if (newCredential) {
                const indexerUrl = getConfig().services.indexerUrl
                const credResponse = await fetch(`${indexerUrl}/service-credentials`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        source_id: source.id,
                        provider: 'zendesk',
                        ...newCredential,
                        config: {},
                    }),
                })
                if (!credResponse.ok) {
                    throw new Error(`Failed to update credentials (HTTP ${credResponse.status})`)
                }
                const credResult = await credResponse.json()
                if (!credResult.success) {
                    throw new Error(credResult.message ?? 'Failed to update credentials')
                }
            }

            if (isActive) {
                const connectorManagerUrl = getConfig().services.connectorManagerUrl
                try {
                    await fetch(`${connectorManagerUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for Zendesk source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Zendesk settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
