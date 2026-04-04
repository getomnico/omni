import { env } from '$env/dynamic/private'
import { json } from '@sveltejs/kit'
import type { RequestHandler } from './$types'
import { getRedisClient } from '$lib/server/redis'
import { db } from '$lib/server/db'
import { sql } from 'drizzle-orm'

interface ServiceHealth {
    status: 'ok' | 'error'
    latency_ms?: number
    error?: string
}

async function checkService(
    name: string,
    url: string,
    path: string,
): Promise<[string, ServiceHealth]> {
    const start = Date.now()
    try {
        const controller = new AbortController()
        const timeout = setTimeout(() => controller.abort(), 5000)
        const response = await fetch(`${url}${path}`, { signal: controller.signal })
        clearTimeout(timeout)
        return [
            name,
            {
                status: response.ok ? 'ok' : 'error',
                latency_ms: Date.now() - start,
                ...(response.ok ? {} : { error: `HTTP ${response.status}` }),
            },
        ]
    } catch (error) {
        return [
            name,
            {
                status: 'error',
                latency_ms: Date.now() - start,
                error: error instanceof Error ? error.message : 'Connection failed',
            },
        ]
    }
}

export const GET: RequestHandler = async () => {
    const results: Record<string, ServiceHealth> = {}

    // Check all services in parallel
    const checks = await Promise.allSettled([
        // Postgres
        (async (): Promise<[string, ServiceHealth]> => {
            const start = Date.now()
            try {
                await db.execute(sql`SELECT 1`)
                return ['postgres', { status: 'ok', latency_ms: Date.now() - start }]
            } catch (error) {
                return [
                    'postgres',
                    {
                        status: 'error',
                        latency_ms: Date.now() - start,
                        error: error instanceof Error ? error.message : 'Connection failed',
                    },
                ]
            }
        })(),
        // Redis
        (async (): Promise<[string, ServiceHealth]> => {
            const start = Date.now()
            try {
                const redis = await getRedisClient()
                await redis.ping()
                return ['redis', { status: 'ok', latency_ms: Date.now() - start }]
            } catch (error) {
                return [
                    'redis',
                    {
                        status: 'error',
                        latency_ms: Date.now() - start,
                        error: error instanceof Error ? error.message : 'Connection failed',
                    },
                ]
            }
        })(),
        // Searcher
        checkService('searcher', env.SEARCHER_URL, '/health'),
        // Indexer
        checkService('indexer', env.INDEXER_URL, '/health'),
        // Connector Manager
        checkService('connector_manager', env.CONNECTOR_MANAGER_URL, '/health'),
    ])

    for (const check of checks) {
        if (check.status === 'fulfilled') {
            const [name, health] = check.value
            results[name] = health
        }
    }

    const allHealthy = Object.values(results).every((s) => s.status === 'ok')

    return json(
        {
            status: allHealthy ? 'healthy' : 'degraded',
            services: results,
            timestamp: new Date().toISOString(),
        },
        { status: allHealthy ? 200 : 503 },
    )
}
