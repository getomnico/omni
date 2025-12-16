import { drizzle } from 'drizzle-orm/postgres-js'
import postgres from 'postgres'
import * as schema from './schema'
import { database } from '../config'

const client = postgres(database.url, {
    max: 10,
    idle_timeout: 20,
    connect_timeout: 10,
})

export const db = drizzle(client, { schema })

process.on('SIGTERM', () => client.end())
process.on('SIGINT', () => client.end())
