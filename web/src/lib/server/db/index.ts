import { drizzle } from 'drizzle-orm/postgres-js'
import postgres from 'postgres'
import * as schema from './schema'
import { database } from '../config'

const adminClient = postgres(database.url, {
    max: 10,
    idle_timeout: 20,
    connect_timeout: 10,
})

export const db = drizzle(adminClient, { schema })

/**
 * Connection pool for RLS queries.
 * Each connection has `app.current_user_id` set before the request.
 * The connection is acquired in hooks.server.ts and released after the request.
 */
export const rlsClient = postgres(database.url, {
    max: 100,
    idle_timeout: 20,
    connect_timeout: 10,
})

process.on('SIGTERM', () => adminClient.end())
process.on('SIGINT', () => adminClient.end())
