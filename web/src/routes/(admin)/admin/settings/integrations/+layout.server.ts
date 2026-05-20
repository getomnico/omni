import { env } from '$env/dynamic/private'
import { sourcesRepository } from '$lib/server/repositories/sources'
import { syncRunsRepository } from '$lib/server/repositories/sync-runs'
import type { LayoutServerLoad } from './$types.js'

const DEFAULT_MAX_CONSECUTIVE_FAILURES = 10

export const load: LayoutServerLoad = async ({ params }) => {
    if (!params.sourceId) {
        return {
            source: null,
            health: 'healthy' as const,
            syncRuns: [],
        }
    }

    const maxConsecutiveFailures =
        Number.parseInt(env.SYNC_MAX_CONSECUTIVE_FAILURES ?? '', 10) ||
        DEFAULT_MAX_CONSECUTIVE_FAILURES
    const syncRunLimit = Math.max(10, maxConsecutiveFailures)
    const [source, syncRuns] = await Promise.all([
        sourcesRepository.getById(params.sourceId),
        syncRunsRepository.getLatestForSourceId(params.sourceId, syncRunLimit),
    ])

    if (!source) {
        return {
            source: null,
            health: 'healthy' as const,
            syncRuns: [],
        }
    }

    const health: 'healthy' | 'unhealthy' = hasFailureStreak(syncRuns, maxConsecutiveFailures)
        ? 'unhealthy'
        : 'healthy'

    return {
        source,
        health,
        syncRuns,
    }
}

function hasFailureStreak(syncRuns: { status: string }[], maxConsecutiveFailures: number) {
    if (maxConsecutiveFailures <= 0) {
        return true
    }

    return (
        syncRuns.length >= maxConsecutiveFailures &&
        syncRuns
            .slice(0, maxConsecutiveFailures)
            .every((run) => run.status.toLowerCase() === 'failed')
    )
}
