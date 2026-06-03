import { expect, test, type Page } from '@playwright/test'
import crypto from 'node:crypto'
import { readFile } from 'node:fs/promises'
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

const streamedMarkdown = `Here are the tools I have available to call right now:\n\n### Search & Retrieval\n- **\`search_documents(query, limit, document_id?)\`** — Search indexed documents\n- **\`read_document(document_id)\`** — Read a document`
const branchedTemplateFixture = new URL(
    './fixtures/chat-01KT1M7YAYQBDP6Z46FAY7G42H.json',
    import.meta.url,
)

type SeededChat = {
    userId: string
    chatId: string
    userMessageId: string
    sessionToken: string
    sessionKey: string
}

type TemplateChatMessage = {
    id: string
    parentId: string | null
    messageSeqNum: number
    message: unknown
    contentText: string | null
    createdAt: string
}

function sseMessage(data: unknown): string {
    return `event: message\ndata: ${JSON.stringify(data)}\n\n`
}

function assistantStart(): string {
    return sseMessage({
        type: 'message_start',
        message: {
            id: `msg_${ulid()}`,
            type: 'message',
            role: 'assistant',
            content: [],
            model: 'playwright-model',
            stop_reason: null,
            stop_sequence: null,
            usage: { input_tokens: 1, output_tokens: 1 },
        },
    })
}

function assistantSearchToolEvents(index: number, toolUseId: string, query: string): string[] {
    return [
        sseMessage({
            type: 'content_block_start',
            index,
            content_block: {
                type: 'tool_use',
                id: toolUseId,
                name: 'search_documents',
                input: {},
            },
        }),
        sseMessage({
            type: 'content_block_delta',
            index,
            delta: {
                type: 'input_json_delta',
                partial_json: JSON.stringify({ query, limit: 10 }),
            },
        }),
        sseMessage({ type: 'content_block_stop', index }),
        sseMessage({ type: 'message_stop' }),
        `event: message_id\ndata: ${ulid()}\n\n`,
    ]
}

function searchToolResult(toolUseId: string): string {
    return sseMessage({
        type: 'tool_result',
        tool_use_id: toolUseId,
        content: [
            {
                type: 'search_result',
                title: 'Synthetic search result',
                source: `synthetic://documents/${toolUseId}`,
                source_type: 'slack',
                content: [],
            },
        ],
        is_error: false,
    })
}

function finalAssistantTextSse(text: string): string {
    return [
        assistantStart(),
        sseMessage({
            type: 'content_block_start',
            index: 0,
            content_block: { type: 'text', text: '' },
        }),
        sseMessage({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text } }),
        sseMessage({ type: 'content_block_stop', index: 0 }),
        sseMessage({ type: 'message_stop' }),
        `event: message_id\ndata: ${ulid()}\n\n`,
    ].join('')
}

function delayedMessageIdFollowUpSse(): string {
    const firstToolUseId = 'toolu_delayed_message_id_first'
    const secondToolUseId = 'toolu_delayed_message_id_second'
    const secondAssistantStart = assistantStart()

    return [
        assistantStart(),
        ...assistantSearchToolEvents(
            0,
            firstToolUseId,
            'synthetic recent project status in:team-channel',
        ),
        searchToolResult(firstToolUseId),
        // Reproduce the production ordering that exposed stale second-assistant
        // stream handling: the next assistant response can begin before the
        // previous tool-result save id reaches the browser.
        secondAssistantStart,
        `event: message_id\ndata: ${ulid()}\n\n`,
        ...assistantSearchToolEvents(
            0,
            secondToolUseId,
            'synthetic stakeholder update in:team-channel',
        ),
        searchToolResult(secondToolUseId),
        `event: message_id\ndata: ${ulid()}\n\n`,
        finalAssistantTextSse(
            '## Synthetic Project Summary (Updated with Recent Information)\n\nThe final answer includes both recent status and stakeholder context.',
        ),
        'event: end_of_stream\ndata: {}\n\n',
    ].join('')
}

function mockedChatSse(finalMessageId: string): string {
    return [
        sseMessage({
            type: 'message_start',
            message: {
                id: 'msg_playwright_mock',
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
            delta: { type: 'text_delta', text: streamedMarkdown.slice(0, 80) },
        }),
        sseMessage({
            type: 'content_block_delta',
            index: 0,
            delta: { type: 'text_delta', text: streamedMarkdown.slice(80) },
        }),
        'event: end_of_stream\ndata: {}\n\n',
    ].join('')
}

async function seedChat(): Promise<SeededChat> {
    const sql = postgres(dbConfig)
    const suffix = crypto.randomUUID()
    const userId = ulid()
    const chatId = ulid()
    const userMessageId = ulid()
    const sessionToken = `playwright-session-${suffix}`
    const sessionId = crypto.createHash('sha256').update(sessionToken).digest('hex')
    const sessionKey = `session:${sessionId}`

    await sql.begin(async (tx) => {
        await tx`
            INSERT INTO users (id, email, role, is_active, auth_method, must_change_password)
            VALUES (${userId}, ${`${userId}@example.test`}, 'admin', true, 'magic_link', false)
        `
        await tx`
            INSERT INTO chats (id, user_id, title, is_starred, is_deleted)
            VALUES (${chatId}, ${userId}, 'Playwright streaming chat', false, false)
        `
        await tx`
            INSERT INTO chat_messages (id, chat_id, parent_id, message_seq_num, message, content_text)
            VALUES (
                ${userMessageId},
                ${chatId},
                NULL,
                1,
                ${tx.json({ role: 'user', content: 'What tools can you use?' })},
                'What tools can you use?'
            )
        `
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

    return { userId, chatId, userMessageId, sessionToken, sessionKey }
}

async function seedChatFromTemplateFixture(): Promise<SeededChat> {
    const sql = postgres(dbConfig)
    const suffix = crypto.randomUUID()
    const userId = ulid()
    const chatId = ulid()
    const sessionToken = `playwright-session-${suffix}`
    const sessionId = crypto.createHash('sha256').update(sessionToken).digest('hex')
    const sessionKey = `session:${sessionId}`
    const templateMessages = JSON.parse(
        await readFile(branchedTemplateFixture, 'utf8'),
    ) as TemplateChatMessage[]
    const idMap = new Map(templateMessages.map((message) => [message.id, ulid()]))
    const userMessageId = idMap.get(templateMessages[0].id)!

    await sql.begin(async (tx) => {
        await tx`
            INSERT INTO users (id, email, role, is_active, auth_method, must_change_password)
            VALUES (${userId}, ${`${userId}@example.test`}, 'admin', true, 'magic_link', false)
        `
        await tx`
            INSERT INTO chats (id, user_id, title, is_starred, is_deleted)
            VALUES (${chatId}, ${userId}, 'Playwright branched streaming chat', false, false)
        `

        for (const message of templateMessages) {
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
                    ${idMap.get(message.id)!},
                    ${chatId},
                    ${message.parentId ? idMap.get(message.parentId)! : null},
                    ${message.messageSeqNum},
                    ${tx.json(message.message)},
                    ${message.contentText},
                    ${new Date(message.createdAt)}
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

    return { userId, chatId, userMessageId, sessionToken, sessionKey }
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

test('chat page renders streamed assistant markdown from the SSE stream endpoint', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChat()
        await authenticate(page, seeded)

        await page.route(`**/api/chat/${seeded.chatId}/messages`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ messageId: ulid() }),
            })
        })

        await page.route(`**/api/chat/${seeded.chatId}/stream`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: {
                    'content-type': 'text/event-stream',
                    'cache-control': 'no-cache',
                    connection: 'keep-alive',
                },
                body: mockedChatSse(ulid()),
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)

        await expect(page.getByText('What tools can you use?')).toBeVisible()
        await page.getByRole('main').getByRole('textbox').fill('Please stream the tool list')
        await page.keyboard.press('Enter')

        await expect(page.getByText('Please stream the tool list')).toBeVisible()
        await expect(
            page.getByText('Here are the tools I have available to call right now:'),
        ).toBeVisible()
        await expect(page.getByRole('heading', { name: 'Search & Retrieval' })).toBeVisible()
        await expect(page.getByText('search_documents(query, limit, document_id?)')).toBeVisible()
        await expect(page.getByText('Read a document')).toBeVisible()
    } finally {
        await cleanupChat(seeded)
    }
})

test('branched chat renders streamed tool calls from the SSE stream endpoint', async ({ page }) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChatFromTemplateFixture()
        await authenticate(page, seeded)

        // Use the real messages and stream endpoints here. Playwright config points
        // the stream endpoint at the recorded SSE fixture for this regression flow.
        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('Replay the failing stream')
        await page.keyboard.press('Enter')

        await expect(page.getByText('Replay the failing stream')).toBeVisible()
        await expect(
            page.getByText("I'll search for more documents related to Nepanagar and NEPA."),
        ).toBeVisible()
        await expect(page.locator('.thinking-container')).toBeVisible()

        const earlierStepsButton = page.getByRole('button', { name: /earlier steps?/ })
        await expect(earlierStepsButton).toBeVisible({ timeout: 10_000 })
        await earlierStepsButton.click()
        await expect(page.getByText('searched: Nepanagar NEPA mill township')).toBeVisible()
        await expect(page.getByText(/search for more documents related to Nepanagar/)).toBeVisible()
    } finally {
        await cleanupChat(seeded)
    }
})

test('branched chat keeps the second streamed assistant response on delayed tool-result id', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChatFromTemplateFixture()
        await authenticate(page, seeded)

        await page.route(`**/api/chat/${seeded.chatId}/messages`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ messageId: ulid() }),
            })
        })

        await page.route(`**/api/chat/${seeded.chatId}/stream`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: {
                    'content-type': 'text/event-stream',
                    'cache-control': 'no-cache',
                    connection: 'keep-alive',
                },
                body: delayedMessageIdFollowUpSse(),
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await page
            .getByRole('main')
            .getByRole('textbox')
            .fill(
                'ok great, can you do just one or two more searches to gather the most current information',
            )
        await page.keyboard.press('Enter')

        await expect(
            page.getByText('searched: synthetic recent project status in:team-channel'),
        ).toBeVisible()
        await expect(
            page.getByText('searched: synthetic stakeholder update in:team-channel'),
        ).toBeVisible()
        await expect(
            page.getByRole('heading', {
                name: 'Synthetic Project Summary (Updated with Recent Information)',
            }),
        ).toBeVisible()
    } finally {
        await cleanupChat(seeded)
    }
})
