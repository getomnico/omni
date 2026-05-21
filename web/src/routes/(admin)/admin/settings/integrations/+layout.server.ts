import { getConfig } from '$lib/server/config'
import type { Source, SyncRun } from '$lib/server/db/schema'
import type { LayoutServerLoad } from './$types.js'

interface ConnectorManagerSourceOverview {
    source: Record<string, unknown>
    health: 'healthy' | 'unhealthy'
    sync_runs: Record<string, string | number | null>[]
}

function mapSource(source: Record<string, unknown>): Source {
    return {
        id: source.id as string,
        name: source.name as string,
        sourceType: source.source_type as string,
        config: source.config,
        isActive: source.is_active as boolean,
        isDeleted: source.is_deleted as boolean,
        scope: source.scope as string,
        userFilterMode: source.user_filter_mode as string,
        userWhitelist: source.user_whitelist,
        userBlacklist: source.user_blacklist,
        createdAt: new Date(source.created_at as string),
        updatedAt: new Date(source.updated_at as string),
        createdBy: source.created_by as string,
        syncIntervalSeconds: source.sync_interval_seconds as number | null,
    }
}

function mapSyncRun(run: Record<string, string | number | null>): SyncRun {
    return {
        id: run.id as string,
        sourceId: run.source_id as string,
        syncType: run.sync_type as string,
        startedAt: new Date(run.started_at as string),
        completedAt: run.completed_at ? new Date(run.completed_at as string) : null,
        status: run.status as string,
        documentsScanned: run.documents_scanned as number | null,
        documentsProcessed: run.documents_processed as number | null,
        documentsUpdated: run.documents_updated as number | null,
        errorMessage: run.error_message as string | null,
        createdAt: new Date(run.created_at as string),
        updatedAt: new Date(run.updated_at as string),
    }
}

export const load: LayoutServerLoad = async ({ params }) => {
    if (!params.sourceId) {
        return {
            source: null,
            health: 'healthy' as const,
            syncRuns: [],
        }
    }

    // Sync health/run filtering belongs to connector-manager because it owns the
    // scheduler circuit-breaker semantics. Fetching sync_runs directly here can
    // drift from connector-manager's rules for manual runs, cancelled runs, and
    // realtime sync noise.
    const config = getConfig()
    const overviewResponse = await fetch(
        `${config.services.connectorManagerUrl}/sources/${params.sourceId}`,
    )

    if (!overviewResponse.ok) {
        return {
            source: null,
            health: 'healthy' as const,
            syncRuns: [],
        }
    }

    const overview = (await overviewResponse.json()) as ConnectorManagerSourceOverview

    return {
        source: mapSource(overview.source),
        health: overview.health,
        syncRuns: overview.sync_runs.map(mapSyncRun),
    }
}
