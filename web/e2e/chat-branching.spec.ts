import { expect, test, type Page } from '@playwright/test'
import crypto from 'node:crypto'
import postgres from 'postgres'
import { createClient } from 'redis'
import { ulid } from 'ulid'

const dbConfig = {
    host: process.env.DATABASE_HOST ?? 'localhost',
    port: Number(process.env.DATABASE_PORT ?? '5432'),
    database: process.env.DATABASE_NAME ?? 'omni_dev',
    username: process.env.DATABASE_USERNAME ?? 'omni_dev',
    password: process.env.DATABASE_PASSWORD ?? 'omni_dev_password',
}

const redisUrl = process.env.REDIS_URL ?? 'redis://localhost:6379'
const authSessionCookieName = process.env.SESSION_COOKIE_NAME ?? 'auth-session'

type SeededChat = {
    userId: string
    chatId: string
    sessionToken: string
    sessionKey: string
    messages: Record<string, string>
}

type BranchMessage = {
    key: string
    parentKey: string | null
    role: 'user' | 'assistant'
    content: string
}

function sseMessage(data: unknown): string {
    return `event: message\ndata: ${JSON.stringify(data)}\n\n`
}

function textStream(finalMessageId: string, text: string): string {
    return [
        sseMessage({
            type: 'message_start',
            message: {
                id: 'msg_branching_playwright',
                type: 'message',
                role: 'assistant',
                content: [],
                model: 'playwright-model',
                stop_reason: null,
                stop_sequence: null,
                usage: { input_tokens: 1, output_tokens: 1 },
            },
        }),
        `event: message_id\ndata: ${finalMessageId}\n\n`,
        sseMessage({
            type: 'content_block_start',
            index: 0,
            content_block: { type: 'text', text: '' },
        }),
        sseMessage({
            type: 'content_block_delta',
            index: 0,
            delta: { type: 'text_delta', text },
        }),
        'event: end_of_stream\ndata: {}\n\n',
    ].join('')
}

async function seedBranchChat(messages: BranchMessage[]): Promise<SeededChat> {
    const sql = postgres(dbConfig)
    const suffix = crypto.randomUUID()
    const userId = ulid()
    const chatId = ulid()
    const sessionToken = `playwright-session-${suffix}`
    const sessionId = crypto.createHash('sha256').update(sessionToken).digest('hex')
    const sessionKey = `session:${sessionId}`
    const idByKey = new Map(messages.map((message) => [message.key, ulid()]))
    const createdAt = new Date('2026-01-01T00:00:00.000Z')

    await sql.begin(async (tx) => {
        await tx`
            INSERT INTO users (id, email, role, is_active, auth_method, must_change_password)
            VALUES (${userId}, ${`${userId}@example.test`}, 'admin', true, 'magic_link', false)
        `
        await tx`
            INSERT INTO chats (id, user_id, title, is_starred, is_deleted)
            VALUES (${chatId}, ${userId}, 'Playwright branching chat', false, false)
        `

        for (let index = 0; index < messages.length; index++) {
            const message = messages[index]
            const parentId = message.parentKey ? idByKey.get(message.parentKey)! : null
            await tx`
                INSERT INTO chat_messages (
                    id,
                    chat_id,
                    parent_id,
                    message_seq_num,
                    message,
                    content_text,
                    created_at
                )
                VALUES (
                    ${idByKey.get(message.key)!},
                    ${chatId},
                    ${parentId},
                    ${index + 1},
                    ${tx.json({ role: message.role, content: message.content })},
                    ${message.content},
                    ${new Date(createdAt.getTime() + index * 1000)}
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

    return { userId, chatId, sessionToken, sessionKey, messages: Object.fromEntries(idByKey) }
}

async function countMessagesContaining(
    chatId: string,
    role: 'user' | 'assistant',
    text: string,
): Promise<number> {
    const sql = postgres(dbConfig)
    const [row] = await sql<{ count: number }[]>`
        SELECT COUNT(*)::int AS count
        FROM chat_messages
        WHERE chat_id = ${chatId}
          AND message->>'role' = ${role}
          AND message::text LIKE ${`%${text}%`}
    `
    await sql.end()

    return row.count
}

async function cleanupChat(seeded: SeededChat | null): Promise<void> {
    if (!seeded) return

    const redis = createClient({ url: redisUrl })
    await redis.connect()
    await redis.del(seeded.sessionKey)
    await redis.disconnect()

    const sql = postgres(dbConfig)
    await sql.begin(async (tx) => {
        await tx`DELETE FROM chat_messages WHERE chat_id = ${seeded.chatId}`
        await tx`DELETE FROM chats WHERE id = ${seeded.chatId}`
        await tx`DELETE FROM users WHERE id = ${seeded.userId}`
    })
    await sql.end()
}

async function authenticate(page: Page, seeded: SeededChat): Promise<void> {
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

async function openSeededChat(page: Page, messages: BranchMessage[]): Promise<SeededChat> {
    const seeded = await seedBranchChat(messages)
    await authenticate(page, seeded)
    await page.goto(`/chat/${seeded.chatId}`)
    return seeded
}

function branchNav(page: Page, messageId: string) {
    return page.getByTestId(`branch-nav-${messageId}`)
}

const simpleSiblingTree: BranchMessage[] = [
    { key: 'root', parentKey: null, role: 'user', content: 'root prompt' },
    { key: 'oldAssistant', parentKey: 'root', role: 'assistant', content: 'assistant old branch' },
    {
        key: 'latestAssistant',
        parentKey: 'root',
        role: 'assistant',
        content: 'assistant latest branch',
    },
]

test('branching chat defaults to the latest sibling branch', async ({ page }) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await openSeededChat(page, simpleSiblingTree)

        await expect(page.getByText('root prompt')).toBeVisible()
        await expect(page.getByText('assistant latest branch')).toBeVisible()
        await expect(page.getByText('assistant old branch')).toHaveCount(0)
        await expect(
            branchNav(page, seeded.messages.latestAssistant).getByTestId('branch-position'),
        ).toHaveText('2/2')
    } finally {
        await cleanupChat(seeded)
    }
})

test('branching chat switches visible sibling branch', async ({ page }) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await openSeededChat(page, simpleSiblingTree)

        await branchNav(page, seeded.messages.latestAssistant).getByTestId('branch-prev').click()

        await expect(page.getByText('assistant old branch')).toBeVisible()
        await expect(page.getByText('assistant latest branch')).toHaveCount(0)
        await expect(
            branchNav(page, seeded.messages.oldAssistant).getByTestId('branch-position'),
        ).toHaveText('1/2')
    } finally {
        await cleanupChat(seeded)
    }
})

test('branching chat clears downstream branch selection when changing parent branches', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await openSeededChat(page, [
            { key: 'root', parentKey: null, role: 'user', content: 'root prompt' },
            {
                key: 'assistantA',
                parentKey: 'root',
                role: 'assistant',
                content: 'assistant branch A',
            },
            {
                key: 'userAOlder',
                parentKey: 'assistantA',
                role: 'user',
                content: 'user A older follow-up',
            },
            {
                key: 'assistantAOlder',
                parentKey: 'userAOlder',
                role: 'assistant',
                content: 'assistant A older answer',
            },
            {
                key: 'userALatest',
                parentKey: 'assistantA',
                role: 'user',
                content: 'user A latest follow-up',
            },
            {
                key: 'assistantALatest',
                parentKey: 'userALatest',
                role: 'assistant',
                content: 'assistant A latest answer',
            },
            {
                key: 'assistantB',
                parentKey: 'root',
                role: 'assistant',
                content: 'assistant branch B',
            },
            {
                key: 'userB',
                parentKey: 'assistantB',
                role: 'user',
                content: 'user B follow-up',
            },
            {
                key: 'assistantBChild',
                parentKey: 'userB',
                role: 'assistant',
                content: 'assistant B child answer',
            },
        ])

        await expect(page.getByText('assistant branch B')).toBeVisible()
        await branchNav(page, seeded.messages.assistantB).getByTestId('branch-prev').click()
        await expect(page.getByText('user A latest follow-up')).toBeVisible()

        await branchNav(page, seeded.messages.userALatest).getByTestId('branch-prev').click()
        await expect(page.getByText('user A older follow-up')).toBeVisible()
        await expect(page.getByText('user A latest follow-up')).toHaveCount(0)

        await branchNav(page, seeded.messages.assistantA).getByTestId('branch-next').click()
        await expect(page.getByText('assistant branch B')).toBeVisible()

        await branchNav(page, seeded.messages.assistantB).getByTestId('branch-prev').click()
        await expect(page.getByText('user A latest follow-up')).toBeVisible()
        await expect(page.getByText('assistant A latest answer')).toBeVisible()
        await expect(page.getByText('user A older follow-up')).toHaveCount(0)
        await expect(page.getByText('assistant A older answer')).toHaveCount(0)
    } finally {
        await cleanupChat(seeded)
    }
})

test('retrying a user message regenerates without duplicating the user message', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await openSeededChat(page, [
            { key: 'root', parentKey: null, role: 'user', content: 'retry prompt' },
            { key: 'oldAssistant', parentKey: 'root', role: 'assistant', content: 'old answer' },
        ])
        await page.route(`**/api/chat/${seeded.chatId}/stream**`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: {
                    'content-type': 'text/event-stream',
                    'cache-control': 'no-cache',
                    connection: 'keep-alive',
                },
                body: textStream(ulid(), 'retried answer'),
            })
        })

        await expect(page.getByText('retry prompt')).toBeVisible()
        await expect(page.getByText('old answer')).toBeVisible()

        await page.getByText('retry prompt').hover()
        await page.getByTestId(`retry-message-${seeded.messages.root}`).click({ force: true })

        await expect(page.getByText('retried answer')).toBeVisible()
        await expect(page.getByText('retry prompt')).toHaveCount(1)
        await expect(page.getByText('old answer')).toHaveCount(0)
        await expect
            .poll(() => countMessagesContaining(seeded!.chatId, 'user', 'retry prompt'), {
                message: 'retry should not create a duplicate user message row',
            })
            .toBe(1)
    } finally {
        await cleanupChat(seeded)
    }
})

test('branching chat streams follow-up on the selected non-latest branch', async ({ page }) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await openSeededChat(page, simpleSiblingTree)
        await page.route(`**/api/chat/${seeded.chatId}/stream`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: {
                    'content-type': 'text/event-stream',
                    'cache-control': 'no-cache',
                    connection: 'keep-alive',
                },
                body: textStream(ulid(), 'streamed answer for old branch'),
            })
        })

        await branchNav(page, seeded.messages.latestAssistant).getByTestId('branch-prev').click()
        await expect(page.getByText('assistant old branch')).toBeVisible()

        await page.getByRole('main').getByRole('textbox').fill('follow up on old branch')
        await page.keyboard.press('Enter')

        await expect(page.getByText('follow up on old branch')).toBeVisible()
        await expect(page.getByText('streamed answer for old branch')).toBeVisible()
        await expect(page.getByText('assistant old branch')).toBeVisible()
        await expect(page.getByText('assistant latest branch')).toHaveCount(0)
    } finally {
        await cleanupChat(seeded)
    }
})
