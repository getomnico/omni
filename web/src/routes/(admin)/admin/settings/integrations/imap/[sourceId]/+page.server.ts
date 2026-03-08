import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType, type ImapSourceConfig } from '$lib/types'
import { db } from '$lib/server/db'
import { serviceCredentials } from '$lib/server/db/schema'
import { eq } from 'drizzle-orm'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.IMAP) {
        throw error(400, 'Invalid source type for this page')
    }

    // Return the principal_email (username) so the form can pre-fill it.
    // The encrypted password is never returned to the client.
    const creds = await db.query.serviceCredentials.findFirst({
        where: eq(serviceCredentials.sourceId, params.sourceId),
    })

    return {
        source,
        principalEmail: creds?.principalEmail ?? null,
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

        if (source.sourceType !== SourceType.IMAP) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const host = (formData.get('host') as string | null)?.trim() ?? ''
        const portStr = formData.get('port') as string | null
        const encryption = (formData.get('encryption') as string | null) ?? 'tls'
        const username = (formData.get('username') as string | null)?.trim() ?? ''
        const password = (formData.get('password') as string | null) ?? ''
        const folderAllowlist = (formData.getAll('folderAllowlist') as string[]).filter(Boolean)
        const folderDenylist = (formData.getAll('folderDenylist') as string[]).filter(Boolean)
        const maxSizeMbStr = formData.get('maxMessageSizeMb') as string | null

        if (!host) {
            throw error(400, 'IMAP server host is required')
        }

        const port = parseInt(portStr ?? '993', 10)
        if (isNaN(port) || port < 1 || port > 65535) {
            throw error(400, 'Port must be between 1 and 65535')
        }

        const maxMessageSizeMb = parseInt(maxSizeMbStr ?? '0', 10) || 0

        try {
            const existingConfig = (source.config as Partial<ImapSourceConfig>) ?? {}

            const config: ImapSourceConfig = {
                display_name: existingConfig.display_name,
                host,
                port,
                encryption,
                folder_allowlist: folderAllowlist,
                folder_denylist: folderDenylist,
                max_message_size: maxMessageSizeMb > 0 ? maxMessageSizeMb * 1024 * 1024 : 0,
                sync_enabled: isActive,
            }

            await updateSourceById(source.id, { isActive, config })

            // If a new password was supplied, re-save the full credential set.
            // The indexer's POST endpoint deletes-then-recreates, so this is
            // also the correct way to rotate a password.
            if (password && username) {
                const indexerUrl = getConfig().services.indexerUrl
                const credResponse = await fetch(`${indexerUrl}/service-credentials`, {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({
                        source_id: source.id,
                        provider: 'imap',
                        auth_type: 'basic_auth',
                        principal_email: username,
                        credentials: { username, password },
                        config: {},
                    }),
                })
                if (!credResponse.ok) {
                    const text = await credResponse.text()
                    throw new Error(`Failed to update credentials: ${text}`)
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
                    console.error(`Failed to trigger sync for IMAP source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save IMAP settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
