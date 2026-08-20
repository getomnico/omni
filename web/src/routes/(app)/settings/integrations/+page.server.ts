import { redirect, fail } from '@sveltejs/kit'
import { getConnectorConfigPublic } from '$lib/server/db/connector-configs'
import { db } from '$lib/server/db'
import { sources, documents } from '$lib/server/db/schema'
import { eq, and, count, inArray } from 'drizzle-orm'
import { updateSourceById } from '$lib/server/db/sources'
import { sourcesRepository } from '$lib/server/repositories/sources'
import {
    getOAuthManifestForSourceType,
    oauthServiceBaseUrl,
} from '$lib/server/oauth/connectorOAuth'
import { SourceType, supportsDataSync } from '$lib/types'
import type { FolderPathFilter } from '$lib/types'
import { getConfig } from '$lib/server/config'
import type { PageServerLoad, Actions } from './$types'

export const load: PageServerLoad = async ({ locals }) => {
    if (!locals.user) {
        throw redirect(302, '/login')
    }

    const googleConnectorConfig = await getConnectorConfigPublic('google')

    // The Windshift server URL is an admin setting stored in connector_configs.
    // Fall back to the connector manifest (env-var based deployments) until the
    // admin configures it in the UI.
    const windshiftConnectorConfig = await getConnectorConfigPublic('windshift')
    const storedWindshiftBaseUrl = windshiftConnectorConfig?.config?.base_url
    let windshiftBaseUrl: string | null =
        typeof storedWindshiftBaseUrl === 'string' && storedWindshiftBaseUrl.trim().length > 0
            ? storedWindshiftBaseUrl
            : null
    if (!windshiftBaseUrl) {
        try {
            const oauth = await getOAuthManifestForSourceType(SourceType.WINDSHIFT)
            if (oauth?.auth_endpoint) windshiftBaseUrl = oauthServiceBaseUrl(oauth.auth_endpoint)
        } catch (err) {
            locals.logger.warn('Failed to load the Windshift connector URL', err)
        }
    }

    const userSources = (await sourcesRepository.getByUserId(locals.user.id)).filter((source) =>
        supportsDataSync(source.integrationType),
    )

    // Load sync status and document counts for personal syncable sources owned by this user
    const userSourceIds = userSources.map((s) => s.id)
    const latestSyncRuns = await sourcesRepository.getLatestSyncRunsForSourceIds(userSourceIds)

    let documentCounts: Record<string, number> = {}
    if (userSourceIds.length > 0) {
        const counts = await db
            .select({
                sourceId: documents.sourceId,
                count: count(),
            })
            .from(documents)
            .where(inArray(documents.sourceId, userSourceIds))
            .groupBy(documents.sourceId)
        for (const row of counts) {
            documentCounts[row.sourceId] = row.count
        }
    }

    return {
        googleOAuthConfigured: !!(
            googleConnectorConfig && googleConnectorConfig.config.oauth_client_id
        ),
        windshiftBaseUrl,
        userSources,
        latestSyncRuns,
        documentCounts,
    }
}

function parsePersonalDriveScope(formData: FormData): {
    indexScope: 'all' | 'selected'
    filters: FolderPathFilter[]
} {
    const indexScope = formData.get('indexScope')
    if (indexScope !== 'all' && indexScope !== 'selected') {
        throw new Error('indexScope must be all or selected')
    }

    const raw = formData.get('folder_path_filters')
    let parsed: unknown = []
    if (typeof raw === 'string' && raw.length > 0) {
        try {
            parsed = JSON.parse(raw)
        } catch {
            throw new Error('folder_path_filters is not valid JSON')
        }
    }
    if (!Array.isArray(parsed)) {
        throw new Error('folder_path_filters must be an array')
    }
    if (parsed.length > 100) {
        throw new Error('At most 100 Drive folders can be selected')
    }

    const seen = new Set<string>()
    const filters: FolderPathFilter[] = []
    for (const value of parsed) {
        if (!value || typeof value !== 'object' || Array.isArray(value)) {
            throw new Error('Each folder filter must be an object')
        }
        const entry = value as Record<string, unknown>
        const allowedKeys = new Set(['id', 'name', 'path', 'driveId', 'kind'])
        if (Object.keys(entry).some((key) => !allowedKeys.has(key))) {
            throw new Error('Folder filters contain an unknown field')
        }
        if (
            typeof entry.id !== 'string' ||
            typeof entry.name !== 'string' ||
            typeof entry.path !== 'string' ||
            typeof entry.driveId !== 'string' ||
            (entry.kind !== 'folder' && entry.kind !== 'shared_drive_root')
        ) {
            throw new Error('Folder filters contain an invalid entry')
        }
        if (!entry.id || !entry.name || !entry.path || !entry.driveId) {
            throw new Error('Folder filters contain an empty value')
        }
        if (seen.has(entry.id)) continue
        seen.add(entry.id)
        filters.push({
            id: entry.id,
            name: entry.name,
            path: entry.path,
            driveId: entry.driveId,
            kind: entry.kind,
        })
    }

    if (indexScope === 'selected' && filters.length === 0) {
        throw new Error('Select at least one Drive folder')
    }
    return { indexScope, filters: indexScope === 'all' ? [] : filters }
}

export const actions: Actions = {
    disable: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const sourceId = formData.get('sourceId') as string
        if (!sourceId) {
            return fail(400, { error: 'Source ID is required' })
        }

        // Verify ownership
        const [source] = await db
            .select()
            .from(sources)
            .where(and(eq(sources.id, sourceId), eq(sources.createdBy, locals.user.id)))
            .limit(1)

        if (!source) {
            return fail(403, { error: 'Source not found or not owned by you' })
        }
        if (!supportsDataSync(source.integrationType)) {
            return fail(400, { error: 'Source does not support data sync' })
        }

        await updateSourceById(sourceId, { isActive: false })
    },

    enable: async ({ request, locals }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const sourceId = formData.get('sourceId') as string
        if (!sourceId) {
            return fail(400, { error: 'Source ID is required' })
        }

        // Verify ownership
        const [source] = await db
            .select()
            .from(sources)
            .where(and(eq(sources.id, sourceId), eq(sources.createdBy, locals.user.id)))
            .limit(1)

        if (!source) {
            return fail(403, { error: 'Source not found or not owned by you' })
        }
        if (!supportsDataSync(source.integrationType)) {
            return fail(400, { error: 'Source does not support data sync' })
        }

        const sourceConfig =
            source.config && typeof source.config === 'object' && !Array.isArray(source.config)
                ? (source.config as Record<string, unknown>)
                : {}
        if (
            source.sourceType === SourceType.GOOGLE_DRIVE &&
            sourceConfig.index_scope === 'pending'
        ) {
            return fail(400, { error: 'Choose a Google Drive indexing scope first' })
        }

        await updateSourceById(sourceId, { isActive: true })
    },

    configureDrive: async ({ request, locals, fetch }) => {
        if (!locals.user) {
            throw redirect(302, '/login')
        }

        const formData = await request.formData()
        const sourceId = formData.get('sourceId') as string
        if (!sourceId) return fail(400, { error: 'Source ID is required' })
        const [source] = await db
            .select()
            .from(sources)
            .where(
                and(
                    eq(sources.id, sourceId),
                    eq(sources.createdBy, locals.user.id),
                    eq(sources.scope, 'user'),
                    eq(sources.isDeleted, false),
                    eq(sources.sourceType, SourceType.GOOGLE_DRIVE),
                ),
            )
            .limit(1)
        if (!source) return fail(404, { error: 'Google Drive source not found' })

        let scope: ReturnType<typeof parsePersonalDriveScope>
        try {
            scope = parsePersonalDriveScope(formData)
        } catch (err) {
            return fail(400, { error: err instanceof Error ? err.message : 'Invalid Drive scope' })
        }

        const existingConfig =
            source.config && typeof source.config === 'object' && !Array.isArray(source.config)
                ? (source.config as Record<string, unknown>)
                : {}
        const nextConfig = {
            ...existingConfig,
            index_scope: scope.indexScope,
            folder_path_filters: scope.filters,
        }

        await updateSourceById(source.id, { isActive: true, config: nextConfig })

        try {
            const response = await fetch(`${getConfig().services.connectorManagerUrl}/sync`, {
                method: 'POST',
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ source_id: source.id, sync_mode: 'full' }),
            })
            if (!response.ok && response.status !== 409) {
                return fail(502, {
                    error: 'Drive scope saved, but the full sync could not start',
                    sourceId: source.id,
                })
            }
        } catch {
            return fail(502, {
                error: 'Drive scope saved, but the full sync could not start',
                sourceId: source.id,
            })
        }

        return { success: true, sourceId: source.id }
    },
}
