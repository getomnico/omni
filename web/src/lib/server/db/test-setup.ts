import { PostgreSqlContainer, type StartedPostgreSqlContainer } from '@testcontainers/postgresql'
import { drizzle, type PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import postgres from 'postgres'
import * as fs from 'fs'
import * as path from 'path'
import * as schema from './schema'
import { ulid } from 'ulid'

let container: StartedPostgreSqlContainer
let sql: ReturnType<typeof postgres>
let db: PostgresJsDatabase<typeof schema>

export async function startTestDb(): Promise<PostgresJsDatabase<typeof schema>> {
    container = await new PostgreSqlContainer('paradedb/paradedb:0.24.0-pg17').start()

    sql = postgres(container.getConnectionUri(), { max: 5 })
    db = drizzle(sql, { schema })

    await runMigrations(sql)

    return db
}

export async function stopTestDb(): Promise<void> {
    await sql.end()
    await container.stop()
}

export function getTestDb(): PostgresJsDatabase<typeof schema> {
    return db
}

async function runMigrations(sqlClient: ReturnType<typeof postgres>): Promise<void> {
    const migrationsDir = path.resolve(__dirname, '../../../../../services/migrations')
    const files = fs
        .readdirSync(migrationsDir)
        .filter((f) => f.endsWith('.sql'))
        .sort()

    for (const file of files) {
        const content = fs.readFileSync(path.join(migrationsDir, file), 'utf-8')
        try {
            await sqlClient.unsafe(content)
        } catch (err: any) {
            console.warn(`Migration ${file} failed: ${err.message}`)
        }
    }
}

export async function createTestUser(database: PostgresJsDatabase<typeof schema>): Promise<string> {
    const userId = ulid()
    await database.insert(schema.user).values({
        id: userId,
        email: `test-${userId}@example.com`,
        passwordHash: 'test-hash',
        role: 'user',
    })
    return userId
}

export async function createTestChat(
    database: PostgresJsDatabase<typeof schema>,
    userId: string,
    title?: string,
): Promise<string> {
    const chatId = ulid()
    await database.insert(schema.chats).values({
        id: chatId,
        userId,
        title: title || 'Test Chat',
    })
    return chatId
}
