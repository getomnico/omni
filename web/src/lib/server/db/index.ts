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

export const documentRlsClient = postgres(database.url, {
    max: 20,
    idle_timeout: 20,
    connect_timeout: 10,
})

process.on('SIGTERM', () => Promise.all([client.end(), documentRlsClient.end()]))
process.on('SIGINT', () => Promise.all([client.end(), documentRlsClient.end()]))
