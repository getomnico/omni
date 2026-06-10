import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { decryptConfig } from '$lib/server/crypto/encryption'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { SourceType } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.SLACK) {
        throw error(400, 'Invalid source type for this page')
    }

    return {
        source,
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

        if (source.sourceType !== SourceType.SLACK) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()
        const isActive = formData.has('enabled')
        const botToken = ((formData.get('botToken') as string | null) ?? '').trim()
        const appToken = ((formData.get('appToken') as string | null) ?? '').trim()

        if (botToken && !botToken.startsWith('xoxb-')) {
            throw error(400, 'Bot token must start with xoxb-')
        }

        if (appToken && !appToken.startsWith('xapp-')) {
            throw error(400, 'App-Level Token must start with xapp-')
        }

        const credentialUpdates: Record<string, string> = {}
        if (botToken) {
            credentialUpdates.bot_token = botToken
        }
        if (appToken) {
            credentialUpdates.app_token = appToken
        }

        let mergedCredentials: Record<string, unknown> | null = null
        if (Object.keys(credentialUpdates).length > 0) {
            const existingCredentials = await serviceCredentialsRepository.getOrgCredsBySourceId(
                source.id,
            )

            if (!existingCredentials) {
                throw error(404, 'No Slack credentials exist for this source')
            }

            try {
                mergedCredentials = {
                    ...decryptConfig(existingCredentials.credentials),
                    ...credentialUpdates,
                }
            } catch (err) {
                console.error(`Failed to decrypt Slack credentials for source ${source.id}:`, err)
                throw error(500, 'Failed to read existing Slack credentials')
            }
        }

        try {
            await updateSourceById(source.id, {
                isActive,
                config: source.config || {},
            })

            if (mergedCredentials) {
                const updatedCredentials = await serviceCredentialsRepository.updateBySourceId(
                    source.id,
                    {
                        credentials: mergedCredentials,
                    },
                )

                if (!updatedCredentials) {
                    throw new Error('Failed to update Slack credentials')
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
                    console.error(`Failed to trigger sync for source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Slack settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
