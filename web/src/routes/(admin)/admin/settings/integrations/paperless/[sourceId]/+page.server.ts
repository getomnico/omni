import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType, type PaperlessNgxSourceConfig } from '$lib/types'
import { db } from '$lib/server/db'
import { serviceCredentials } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.PAPERLESS_NGX) {
        throw error(400, 'Invalid source type for this page')
    }

    return { source }
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

        if (source.sourceType !== SourceType.PAPERLESS_NGX) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const baseUrl = (formData.get('base_url') as string | null)?.trim().replace(/\/$/, '') ?? ''
        const apiKey = (formData.get('api_key') as string | null) ?? ''

        if (!baseUrl) {
            throw error(400, 'Paperless-ngx URL is required')
        }

        try {
            const config: PaperlessNgxSourceConfig = {
                base_url: baseUrl,
                sync_enabled: isActive,
            }

            await updateSourceById(source.id, { isActive, config })

            if (apiKey) {
                const indexerUrl = getConfig().services.indexerUrl
                const credResponse = await fetch(`${indexerUrl}/service-credentials`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        source_id: source.id,
                        provider: 'paperless_ngx',
                        auth_type: 'api_key',
                        credentials: { api_key: apiKey },
                        config: {},
                    }),
                })
                if (!credResponse.ok) {
                    const text = await credResponse.text()
                    throw new Error(`Failed to update API key: ${text}`)
                }
                const credResult = await credResponse.json()
                if (!credResult.success) {
                    throw new Error(credResult.message ?? 'Failed to update API key')
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
                    console.error(
                        `Failed to trigger sync for Paperless-ngx source ${source.id}:`,
                        err,
                    )
                }
            }
        } catch (err) {
            console.error('Failed to save Paperless-ngx settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
