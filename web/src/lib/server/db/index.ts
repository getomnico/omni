import { drizzle } from 'drizzle-orm/postgres-js'
import postgres from 'postgres'
import * as schema from './schema'
import { database } from '../config'

const client = postgres(database.url)

export const db = drizzle(client, { schema })
