import { error, redirect } from '@sveltejs/kit'
import type { PageServerLoad, Actions } from './$types'
import { requireAdmin } from '$lib/server/authHelpers'
import { getSourceById, updateSourceById } from '$lib/server/db/sources'
import { getConfig } from '$lib/server/config'
import { SourceType, type FilesystemSourceConfig } from '$lib/types'

export const load: PageServerLoad = async ({ params, locals }) => {
    requireAdmin(locals)

    const source = await getSourceById(params.sourceId)

    if (!source) {
        throw error(404, 'Source not found')
    }

    if (source.sourceType !== SourceType.LOCAL_FILES) {
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

        if (source.sourceType !== SourceType.LOCAL_FILES) {
            throw error(400, 'Invalid source type')
        }

        const formData = await request.formData()

        const isActive = formData.has('filesystemEnabled')
        const basePath = (formData.get('basePath') as string) || ''
        const fileExtensions = formData.getAll('fileExtensions') as string[]
        const excludePatterns = formData.getAll('excludePatterns') as string[]
        const maxFileSizeMb = parseInt(formData.get('maxFileSizeMb') as string) || 10
        const scanIntervalSeconds = parseInt(formData.get('scanIntervalSeconds') as string) || 300

        if (isActive && !basePath.trim()) {
            throw error(400, 'Base path is required when filesystem indexing is enabled')
        }

        if (isActive && !basePath.startsWith('/')) {
            throw error(400, 'Base path must be an absolute path (starting with /)')
        }

        try {
            const config: FilesystemSourceConfig = {
                base_path: basePath,
                file_extensions: fileExtensions.length > 0 ? fileExtensions : undefined,
                exclude_patterns: excludePatterns.length > 0 ? excludePatterns : undefined,
                max_file_size_bytes: maxFileSizeMb * 1024 * 1024,
                scan_interval_seconds: scanIntervalSeconds,
            }

            await updateSourceById(source.id, {
                isActive,
                config,
            })

            if (isActive) {
                const connectorManagerUrl = getConfig().services.connectorManagerUrl
                try {
                    await fetch(`${connectorManagerUrl}/sync/${source.id}`, {
                        method: 'POST',
                        headers: { 'Content-Type': 'application/json' },
                    })
                } catch (err) {
                    console.error(`Failed to trigger sync for filesystem source ${source.id}:`, err)
                }
            }
        } catch (err) {
            console.error('Failed to save Filesystem settings:', err)
            throw error(500, 'Failed to save configuration')
        }

        throw redirect(303, '/admin/settings/integrations?success=filesystem_configured')
    },
}
