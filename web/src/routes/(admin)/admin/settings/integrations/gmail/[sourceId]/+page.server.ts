import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { updateSourceById, type UserFilterMode } from '$lib/server/db/sources'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { userRepository } from '$lib/server/db/users'
import { getConfig } from '$lib/server/config'
import { AuthType, SourceType } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await sourcesRepository.getById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    const creator = await userRepository.findById(source.createdBy)
    if (creator?.role !== 'admin') {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.GMAIL) {
        throw error(400, 'Invalid source type for this page')
    }

    const creds = await serviceCredentialsRepository.getOrgCredsBySourceId(source.id)

    const credsConfig = (creds?.config as { domain?: string } | null) ?? {}
    const sourceConfig = (source.config as { domain?: string } | null) ?? {}

    const driveSibling = await sourcesRepository.findActiveByTypeAndCreator(
        SourceType.GOOGLE_DRIVE,
        source.createdBy,
    )

    return {
        source,
        authType: (creds?.authType as AuthType | undefined) ?? null,
        hasStoredKey: Boolean(creds),
        principalEmail: creds?.principalEmail ?? '',
        domain: credsConfig.domain ?? sourceConfig.domain ?? '',
        driveSiblingId: driveSibling?.id ?? null,
    }
}

export const actions: Actions = {
    default: async ({ request, params, locals, fetch }) => {
        const user = locals.user
        if (!user || user.role !== 'admin') {
            throw error(403, 'Admin access required')
        }

        const source = await sourcesRepository.getById(params.sourceId)
        if (!source) {
            throw error(404, 'Source not found')
        }

        const creator = await userRepository.findById(source.createdBy)
        if (creator?.role !== 'admin') {
            throw error(404, 'Source not found')
        }

        if (source.sourceType !== SourceType.GMAIL) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('enabled')
        const userFilterMode = (formData.get('userFilterMode') as UserFilterMode) || 'all'
        const userWhitelist =
            userFilterMode === 'whitelist' ? (formData.getAll('userWhitelist') as string[]) : null
        const userBlacklist =
            userFilterMode === 'blacklist' ? (formData.getAll('userBlacklist') as string[]) : null

        const existingCreds = await serviceCredentialsRepository.getOrgCredsBySourceId(source.id)
        const isJwt = existingCreds?.authType === AuthType.JWT

        try {
            if (isJwt) {
                const serviceAccountJson = (
                    (formData.get('serviceAccountJson') as string) || ''
                ).trim()
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

                if (serviceAccountJson) {
                    try {
                        JSON.parse(serviceAccountJson)
                    } catch {
                        throw error(400, 'Invalid service account JSON')
                    }
                }

                await serviceCredentialsRepository.updateBySourceId(source.id, {
                    principalEmail,
                    config: { domain },
                    credentials: serviceAccountJson
                        ? { service_account_key: serviceAccountJson }
                        : null,
                })

                await updateSourceById(source.id, {
                    isActive,
                    userFilterMode,
                    userWhitelist,
                    userBlacklist,
                    config: { domain },
                })
            } else {
                // OAuth or other auth types — admin can only toggle enabled.
                await updateSourceById(source.id, { isActive })
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
            console.error('Failed to save Gmail settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
