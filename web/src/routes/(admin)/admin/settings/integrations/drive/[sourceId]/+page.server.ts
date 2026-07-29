import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { updateSourceById, type UserFilterMode } from '$lib/server/db/sources'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { serviceCredentialsRepository } from '$lib/server/repositories/service-credentials'
import { userRepository } from '$lib/server/db/users'
import { getConfig } from '$lib/server/config'
import { AuthType, SourceType } from '$lib/types'
import type { FolderPathFilter } from '$lib/types'

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

    if (source.sourceType !== SourceType.GOOGLE_DRIVE) {
        throw error(400, 'Invalid source type for this page')
    }

    const creds = await serviceCredentialsRepository.getOrgCredsBySourceId(source.id)

    const credsConfig = (creds?.config as { domain?: string } | null) ?? {}
    const sourceConfig =
        (source.config as { domain?: string; folder_path_filters?: unknown } | null) ?? {}

    const folderPathFilters = Array.isArray(sourceConfig?.folder_path_filters)
        ? sourceConfig.folder_path_filters
        : []

    const gmailSibling = await sourcesRepository.findActiveByTypeAndCreator(
        SourceType.GMAIL,
        source.createdBy,
    )

    return {
        source,
        authType: (creds?.authType as AuthType | undefined) ?? null,
        hasStoredKey: Boolean(creds),
        principalEmail: creds?.principalEmail ?? '',
        domain: credsConfig.domain ?? sourceConfig.domain ?? '',
        gmailSiblingId: gmailSibling?.id ?? null,
        folderPathFilters: folderPathFilters as FolderPathFilter[],
    }
}

function parseFolderPathFilters(formData: FormData): Record<string, unknown>[] {
    const raw = formData.get('folder_path_filters') as string | null
    if (!raw) return []
    let parsed: unknown
    try {
        parsed = JSON.parse(raw)
    } catch {
        throw error(400, 'folder_path_filters is not valid JSON')
    }
    if (!Array.isArray(parsed)) {
        throw error(400, 'folder_path_filters must be a JSON array')
    }
    // Each entry must have all required fields with correct types and no unknown fields.
    const allowedEntryKeys = ['id', 'name', 'path', 'driveId', 'kind']
    const seenIds = new Set<string>()
    for (const entry of parsed) {
        if (!entry || typeof entry !== 'object') {
            throw error(400, 'Each folder filter entry must be a non-null object')
        }
        const e = entry as Record<string, unknown>
        // Reject unknown fields.
        for (const key of Object.keys(e)) {
            if (!allowedEntryKeys.includes(key)) {
                throw error(400, `Unknown field '${key}' in folder filter entry`)
            }
        }
        // Validate required string fields.
        if (typeof e.id !== 'string' || !e.id) {
            throw error(400, 'Each folder filter entry must have a non-empty id')
        }
        if (typeof e.name !== 'string' || !e.name) {
            throw error(400, 'Each folder filter entry must have a non-empty name')
        }
        if (typeof e.path !== 'string' || !e.path) {
            throw error(400, 'Each folder filter entry must have a non-empty path')
        }
        if (typeof e.driveId !== 'string' || !e.driveId) {
            throw error(400, 'Each folder filter entry must have a non-empty driveId')
        }
        if (e.kind !== 'shared_drive_root' && e.kind !== 'folder') {
            throw error(
                400,
                "Each folder filter entry must have kind 'shared_drive_root' or 'folder'",
            )
        }
        // Deduplicate by stable ID (first-wins).
        if (seenIds.has(e.id)) {
            continue
        }
        seenIds.add(e.id)
    }
    // Return deduplicated array — first-wins.
    const deduplicated: Record<string, unknown>[] = []
    seenIds.clear()
    for (const entry of parsed) {
        const e = entry as Record<string, unknown>
        const id = e.id as string
        if (seenIds.has(id)) continue
        seenIds.add(id)
        deduplicated.push(e)
    }
    return deduplicated
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

                const folderPathFilters = parseFolderPathFilters(formData)

                // Merge folder_path_filters into config while PRESERVING all existing source config keys.
                const existingConfig: Record<string, unknown> =
                    source.config &&
                    typeof source.config === 'object' &&
                    !Array.isArray(source.config)
                        ? (source.config as Record<string, unknown>)
                        : {}

                const mergedConfig: Record<string, unknown> = { ...existingConfig, domain }
                mergedConfig.folder_path_filters = folderPathFilters

                // Preserve existing credential config as well.
                const existingCredConfig: Record<string, unknown> =
                    existingCreds?.config &&
                    typeof existingCreds.config === 'object' &&
                    !Array.isArray(existingCreds.config)
                        ? (existingCreds.config as Record<string, unknown>)
                        : {}

                await serviceCredentialsRepository.updateBySourceId(source.id, {
                    principalEmail,
                    config: { ...existingCredConfig, domain },
                    credentials: serviceAccountJson
                        ? { service_account_key: serviceAccountJson }
                        : null,
                })

                await updateSourceById(source.id, {
                    isActive,
                    userFilterMode,
                    userWhitelist,
                    userBlacklist,
                    config: mergedConfig,
                })
            } else {
                // OAuth or other auth types — admin can only toggle enabled.
                // Still merge any config changes without destroying existing keys.
                const existingConfig: Record<string, unknown> =
                    source.config &&
                    typeof source.config === 'object' &&
                    !Array.isArray(source.config)
                        ? (source.config as Record<string, unknown>)
                        : {}
                const folderPathFilters = parseFolderPathFilters(formData)
                const mergedConfig: Record<string, unknown> = { ...existingConfig }
                mergedConfig.folder_path_filters = folderPathFilters
                await updateSourceById(source.id, {
                    isActive,
                    config: mergedConfig,
                })
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
            console.error('Failed to save Google Drive settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations')
    },
}
