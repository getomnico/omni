import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById, type UserFilterMode } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType } from '$lib/types'
import { db } from '$lib/server/db'
import { serviceCredentials, sources } from '$lib/server/db/schema'
import { and, eq } from 'drizzle-orm'
import { encryptConfig } from '$lib/server/crypto/encryption'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.GOOGLE_DRIVE) {
        throw error(400, 'Invalid source type for this page')
    }

    const creds = await db.query.serviceCredentials.findFirst({
        where: eq(serviceCredentials.sourceId, source.id),
    })

    const credsConfig = (creds?.config as { domain?: string } | null) ?? {}
    const sourceConfig = (source.config as { domain?: string } | null) ?? {}

    const gmailSibling = await db.query.sources.findFirst({
        where: and(
            eq(sources.sourceType, SourceType.GMAIL),
            eq(sources.createdBy, source.createdBy),
            eq(sources.isDeleted, false),
        ),
    })

    return {
        source,
        hasStoredKey: Boolean(creds),
        principalEmail: creds?.principalEmail ?? '',
        domain: credsConfig.domain ?? sourceConfig.domain ?? '',
        gmailSiblingId: gmailSibling?.id ?? null,
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

        if (source.sourceType !== SourceType.GOOGLE_DRIVE) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const userFilterMode = (formData.get('userFilterMode') as UserFilterMode) || 'all'
        const userWhitelist =
            userFilterMode === 'whitelist' ? (formData.getAll('userWhitelist') as string[]) : null
        const userBlacklist =
            userFilterMode === 'blacklist' ? (formData.getAll('userBlacklist') as string[]) : null
        const serviceAccountJson = ((formData.get('serviceAccountJson') as string) || '').trim()
        const principalEmail = ((formData.get('principalEmail') as string) || '').trim()
        const domain = ((formData.get('domain') as string) || '').trim()

        if (
            isActive &&
            userFilterMode === 'whitelist' &&
            (!userWhitelist || userWhitelist.length === 0)
        ) {
            throw error(400, 'Whitelist mode requires at least one user')
        }

        if (!principalEmail) {
            throw error(400, 'Admin email is required')
        }
        if (!domain) {
            throw error(400, 'Organization domain is required')
        }

        let parsedKey: Record<string, unknown> | null = null
        if (serviceAccountJson) {
            try {
                parsedKey = JSON.parse(serviceAccountJson)
            } catch {
                throw error(400, 'Invalid service account JSON')
            }
        }

        try {
            const existingCreds = await db.query.serviceCredentials.findFirst({
                where: eq(serviceCredentials.sourceId, source.id),
            })

            if (existingCreds) {
                const updates: Partial<typeof serviceCredentials.$inferInsert> = {
                    principalEmail,
                    config: { domain },
                    updatedAt: new Date(),
                }
                if (parsedKey) {
                    updates.credentials = encryptConfig({ service_account_key: serviceAccountJson })
                }
                await db
                    .update(serviceCredentials)
                    .set(updates)
                    .where(eq(serviceCredentials.sourceId, source.id))
            }

            await updateSourceById(source.id, {
                isActive,
                userFilterMode,
                userWhitelist,
                userBlacklist,
                config: { domain },
            })

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
            console.error('Failed to save Google Drive settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
