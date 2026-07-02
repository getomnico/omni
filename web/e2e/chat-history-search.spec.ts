import { expect, test, type Page } from '@playwright/test'
import crypto from 'node:crypto'
import postgres from 'postgres'
import { createClient } from 'redis'
import { ulid } from 'ulid'

function requiredEnv(name: string): string {
    const value = process.env[name]
    if (!value) throw new Error(`Missing required environment variable: ${name}`)
    return value
}

const dbConfig = {
    host: requiredEnv('DATABASE_HOST'),
    port: Number.parseInt(requiredEnv('DATABASE_PORT'), 10),
    database: requiredEnv('DATABASE_NAME'),
    username: requiredEnv('DATABASE_USERNAME'),
    password: requiredEnv('DATABASE_PASSWORD'),
}

const redisUrl = requiredEnv('REDIS_URL')
const authSessionCookieName = requiredEnv('SESSION_COOKIE_NAME')

type SeededHistory = {
    userId: string
    chatIds: string[]
    sessionToken: string
    sessionKey: string
}

async function seedHistory(): Promise<SeededHistory> {
    const sql = postgres(dbConfig)
    const suffix = crypto.randomUUID()
    const userId = ulid()
    const sessionToken = `playwright-session-${suffix}`
    const sessionId = crypto.createHash('sha256').update(sessionToken).digest('hex')
    const sessionKey = `session:${sessionId}`
    const chatIds = Array.from({ length: 25 }, () => ulid())
    const now = new Date()

    await sql.begin(async (tx) => {
        await tx`
            INSERT INTO users (id, email, role, is_active, auth_method, must_change_password)
            VALUES (${userId}, ${`${userId}@example.test`}, 'admin', true, 'magic_link', false)
        `

        for (let index = 0; index < chatIds.length; index++) {
            const updatedAt = new Date(now.getTime() - index * 24 * 60 * 60 * 1000)
            const title = index === 1 ? 'Starred Spotlightstar Chat' : `History Chat ${index + 1}`
            await tx`
                INSERT INTO chats (id, user_id, title, is_starred, is_deleted, created_at, updated_at)
                VALUES (
                    ${chatIds[index]},
                    ${userId},
                    ${title},
                    ${index === 1},
                    false,
                    ${updatedAt},
                    ${updatedAt}
                )
            `
        }
    })
    await sql.end()

    const redis = createClient({ url: redisUrl })
    await redis.connect()
    await redis.setEx(
        sessionKey,
        60 * 10,
        JSON.stringify({ id: sessionId, userId, expiresAt: new Date(Date.now() + 60 * 10 * 1000) }),
    )
    await redis.disconnect()

    return { userId, chatIds, sessionToken, sessionKey }
}

async function authenticate(page: Page, seeded: SeededHistory): Promise<void> {
    await page.context().addCookies([
        {
            name: authSessionCookieName,
            value: seeded.sessionToken,
            domain: 'localhost',
            path: '/',
            httpOnly: true,
            sameSite: 'Lax',
            expires: Math.floor(Date.now() / 1000) + 60 * 10,
        },
    ])
}

async function cleanupHistory(seeded: SeededHistory | null): Promise<void> {
    if (!seeded) return

    const redis = createClient({ url: redisUrl })
    await redis.connect()
    await redis.del(seeded.sessionKey)
    await redis.disconnect()

    const sql = postgres(dbConfig)
    await sql.begin(async (tx) => {
        await tx`DELETE FROM chats WHERE user_id = ${seeded.userId}`
        await tx`DELETE FROM users WHERE id = ${seeded.userId}`
    })
    await sql.end()
}

test('sidebar loads older recent chats and spotlight searches starred chats', async ({ page }) => {
    let seeded: SeededHistory | null = null

    try {
        seeded = await seedHistory()
        await authenticate(page, seeded)
        await page.goto('/')

        await expect(page.getByRole('button', { name: /Load more/i })).toBeVisible()
        await page.getByRole('button', { name: /Load more/i }).click()
        const oldestChat = page.getByRole('link', { name: /History Chat 25/i })
        await expect(oldestChat).toHaveCount(1)
        await oldestChat.scrollIntoViewIfNeeded()
        await expect(oldestChat).toBeVisible()

        await page.getByRole('button', { name: 'Search chats' }).click()
        await expect(page.getByPlaceholder('Search chats...')).toBeFocused()

        const searchPopover = page.getByTestId('chat-history-search-popover')
        await expect(searchPopover).toBeVisible()
        const popoverBox = await searchPopover.boundingBox()
        const viewport = page.viewportSize()
        expect(popoverBox).not.toBeNull()
        expect(viewport).not.toBeNull()
        expect(
            Math.abs(popoverBox!.x + popoverBox!.width / 2 - viewport!.width / 2),
        ).toBeLessThanOrEqual(2)
        expect(
            Math.abs(popoverBox!.y + popoverBox!.height / 2 - viewport!.height / 2),
        ).toBeLessThanOrEqual(2)
        expect(Math.abs(popoverBox!.height - viewport!.height / 3)).toBeLessThanOrEqual(2)

        await page.getByPlaceholder('Search chats...').fill('spotlightstar')
        await expect(
            page.getByRole('option', { name: /Starred Spotlightstar Chat/i }),
        ).toBeVisible()
        const populatedPopoverBox = await searchPopover.boundingBox()
        expect(populatedPopoverBox).not.toBeNull()
        expect(Math.abs(populatedPopoverBox!.height - viewport!.height / 3)).toBeLessThanOrEqual(2)
        await expect(page.getByRole('button', { name: /Load more/i })).toHaveCount(0)
    } finally {
        await cleanupHistory(seeded)
    }
})
