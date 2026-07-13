import { expect, test, type Page } from '@playwright/test'
import crypto from 'node:crypto'
import { readFile, unlink, writeFile } from 'node:fs/promises'
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
const capturedSeedChatPath = process.env.OMNI_CAPTURE_SEED_CHAT_PATH
const capturedExpectedText = process.env.OMNI_CAPTURE_EXPECT_TEXT
const capturedSubmitText = process.env.OMNI_CAPTURE_SUBMIT_TEXT

const streamedMarkdown = `Here are the tools I have available to call right now:\n\n### Search & Retrieval\n- **\`search_documents(query, limit, document_id?)\`** — Search indexed documents\n- **\`read_document(document_id)\`** — Read a document`
const branchedTemplateFixture = new URL(
    './fixtures/chat-01KT1M7YAYQBDP6Z46FAY7G42H.json',
    import.meta.url,
)
const capturedSearchesTemplateFixture = new URL(
    './fixtures/chat-captured-searches-seed.json',
    import.meta.url,
)
const citationRenderingTemplateFixture = new URL(
    './fixtures/citation-rendering-chat.json',
    import.meta.url,
)
const replayFixtureCookieName = 'omni-chat-stream-replay-fixture'

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

type InterruptedToolRepairRow = {
    id: string
    parent_id: string | null
    message_seq_num: number
    message: unknown
}

type InterruptedToolResultMessage = {
    role: 'user'
    content: Array<{
        type: 'tool_result'
        tool_use_id: string
        is_error: boolean
        content: Array<{ type: 'text'; text: string }>
    }>
}

type TextUserMessage = {
    role: 'user'
    content: string
}

type SeededCitationChat = SeededChat & {
    assistantMessageId: string
    citationFreeAssistantMessageId: string
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

type ApprovalPauseFixture = {
    approvalId: string
    toolCallId: string
    toolName: string
    toolInput: Record<string, unknown>
}

function approvalRequiredSse({
    approvalId,
    toolCallId,
    toolName,
    toolInput,
}: ApprovalPauseFixture): string {
    return `event: approval_required\ndata: ${JSON.stringify({
        approval_id: approvalId,
        tool_name: toolName,
        tool_input: toolInput,
        tool_call_id: toolCallId,
    })}\n\n`
}

function approvalPauseSse(fixture: ApprovalPauseFixture): string {
    const assistantMessage = {
        role: 'assistant',
        content: [
            {
                type: 'tool_use',
                id: fixture.toolCallId,
                name: fixture.toolName,
                input: fixture.toolInput,
            },
        ],
    }

    return [
        assistantStart(),
        sseMessage({
            type: 'content_block_start',
            index: 0,
            content_block: {
                type: 'tool_use',
                id: fixture.toolCallId,
                name: fixture.toolName,
                input: {},
            },
        }),
        sseMessage({
            type: 'content_block_delta',
            index: 0,
            delta: {
                type: 'input_json_delta',
                partial_json: JSON.stringify(fixture.toolInput),
            },
        }),
        sseMessage({ type: 'content_block_stop', index: 0 }),
        sseMessage({ type: 'message_stop' }),
        `event: save_message\ndata: ${JSON.stringify(assistantMessage)}\n\n`,
        approvalRequiredSse(fixture),
        'event: end_of_stream\ndata: Approval required\n\n',
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

async function seedInterruptedToolCallChat(): Promise<
    SeededChat & { assistantMessageId: string; toolUseId: string }
> {
    const seeded = await seedChat()
    const sql = postgres(dbConfig)
    const assistantMessageId = ulid()
    const toolUseId = `toolu_interrupted_${ulid()}`

    await sql`
        INSERT INTO chat_messages (id, chat_id, parent_id, message_seq_num, message, content_text)
        VALUES (
            ${assistantMessageId},
            ${seeded.chatId},
            ${seeded.userMessageId},
            2,
            ${sql.json({
                role: 'assistant',
                content: [
                    {
                        type: 'tool_use',
                        id: toolUseId,
                        name: 'search_documents',
                        input: { query: 'interrupted tool call', limit: 10 },
                    },
                ],
            })},
            NULL
        )
    `
    await sql.end()

    return { ...seeded, assistantMessageId, toolUseId }
}

async function seedChatFromTemplateFixture(
    fixturePath: URL | string = branchedTemplateFixture,
): Promise<SeededChat> {
    const sql = postgres(dbConfig)
    const suffix = crypto.randomUUID()
    const userId = ulid()
    const chatId = ulid()
    const sessionToken = `playwright-session-${suffix}`
    const sessionId = crypto.createHash('sha256').update(sessionToken).digest('hex')
    const sessionKey = `session:${sessionId}`
    const templateMessages = JSON.parse(
        await readFile(fixturePath, 'utf8'),
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

async function seedCitationRenderingChat(): Promise<SeededCitationChat> {
    const seeded = await seedChatFromTemplateFixture(citationRenderingTemplateFixture)
    const sql = postgres(dbConfig)
    const [assistantMessage] = await sql<{ id: string }[]>`
        SELECT id FROM chat_messages
        WHERE chat_id = ${seeded.chatId} AND message_seq_num = 2
    `
    const [citationFreeMessage] = await sql<{ id: string }[]>`
        SELECT id FROM chat_messages
        WHERE chat_id = ${seeded.chatId} AND message_seq_num = 4
    `
    await sql.end()

    return {
        ...seeded,
        assistantMessageId: assistantMessage.id,
        citationFreeAssistantMessageId: citationFreeMessage.id,
    }
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

async function selectReplayFixture(page: Page, fixtureName: string): Promise<void> {
    await page.context().addCookies([
        {
            name: replayFixtureCookieName,
            value: fixtureName,
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

test('chat renders citation chips and Sources drawer', async ({ page }) => {
    let seeded: SeededCitationChat | null = null
    try {
        seeded = await seedCitationRenderingChat()
        await authenticate(page, seeded)
        await page.setViewportSize({ width: 1280, height: 360 })

        await page.goto(`/chat/${seeded.chatId}`)

        const message = page.getByTestId(`chat-message-${seeded.assistantMessageId}`)
        await expect(message.getByText('A cited answer')).toBeVisible()
        await expect(message.getByText('It should stay inline')).toBeVisible()
        await expect(message.getByText('A cited bullet')).toBeVisible()
        await expect(message.getByText('A text-only bullet')).toBeVisible()

        // --- Inline chip labels appear ---
        await expect(message.getByText('Citation source', { exact: true }).first()).toBeVisible()
        await expect(message.getByText('Bullet citation source', { exact: true })).toBeVisible()
        await expect(message.getByText('Web Source Example', { exact: true })).toBeVisible()
        await expect(
            message.getByText('Google Doc Research', { exact: true }).first(),
        ).toBeVisible()
        await expect(message.getByText('Academic Paper', { exact: true })).toBeVisible()
        await expect(message.getByText('Web Search Title', { exact: true })).toBeVisible()
        await expect(message.getByText('Another Doc', { exact: true })).toBeVisible()

        // --- Paragraph/list structure preserved ---
        await expect
            .poll(async () =>
                message
                    .locator('p')
                    .first()
                    .evaluate((p) => p.textContent?.replace(/\s+/g, ' ').trim()),
            )
            .toBe('A cited answer Citation source. It should stay inline.')

        await expect
            .poll(async () =>
                message
                    .locator('li')
                    .nth(0)
                    .evaluate((li) => li.textContent?.replace(/\s+/g, ' ').trim()),
            )
            .toBe('A cited bullet Bullet citation source should also stay inline.')

        await expect
            .poll(async () =>
                message
                    .locator('li')
                    .nth(1)
                    .evaluate((li) => li.textContent?.replace(/\s+/g, ' ').trim()),
            )
            .toBe('A text-only bullet should also stay inline too.')

        await expect
            .poll(async () => message.evaluate((el) => (el.textContent ?? '').replace(/\s+/g, ' ')))
            .not.toMatch(/\[\d+\]/)

        // --- Navigable source link semantics ---
        const webLinkChip = message.locator('a[href="https://example.com/cited-web-page"]').first()
        await expect(webLinkChip).toBeVisible()
        await expect(webLinkChip).toHaveAttribute('target', '_blank')
        await expect(webLinkChip.evaluate((el) => el.relList?.toString() ?? '')).toMatch(
            /noreferrer/,
        )
        await expect(webLinkChip.evaluate((el) => el.relList?.toString() ?? '')).toMatch(/noopener/)
        await expect(webLinkChip).not.toHaveAttribute('role', 'button')
        await expect(message.getByRole('link', { name: 'Web Source Example' })).toBeVisible()

        // Non-navigable source is a button with role="button"
        const buttonChips = message.locator('button')
        const citationBtn = buttonChips.filter({ hasText: 'Citation source' }).first()
        await expect(citationBtn).toBeVisible()
        await expect(citationBtn).toHaveAttribute('type', 'button')
        await expect(citationBtn).toHaveAttribute('role', 'button')
        await expect(buttonChips.filter({ hasText: 'Web Source Example' })).toHaveCount(0)

        // Long title truncation
        const longChip = message
            .locator('button, a')
            .filter({ hasText: 'A Very Long Document Title' })
            .first()
        await expect(longChip).toBeVisible()
        const trunc = await longChip.evaluate((el) => {
            const span = el.querySelector('span.truncate')
            if (!span) return { truncated: false }
            return {
                truncated: span.scrollWidth > span.clientWidth,
                overflow: getComputedStyle(span).overflow,
                textOverflow: getComputedStyle(span).textOverflow,
            }
        })
        expect(trunc.truncated).toBe(true)
        expect(trunc.overflow).toBe('hidden')
        expect(trunc.textOverflow).toBe('ellipsis')

        // --- Two Google Doc chips (different excerpts from same doc index 6) ---
        const googleDocChips = buttonChips.filter({ hasText: 'Google Doc Research' })
        await expect(googleDocChips).toHaveCount(2)

        // First Google Doc chip hover shows its own snippet
        const hc = page.locator('[data-slot="hover-card-content"]:visible')
        const googleFirst = googleDocChips.first()
        await googleFirst.hover()
        await expect(hc.getByText('Google Doc Research key finding excerpt')).toBeVisible({
            timeout: 2000,
        })
        await expect(hc.getByText('Files', { exact: true })).toBeVisible()
        await expect(hc.getByText('Document excerpt')).toBeVisible()

        // Second Google Doc chip hover shows different snippet
        const googleSecond = googleDocChips.nth(1)
        await googleSecond.hover()
        await expect(hc.getByText('Second Google Doc excerpt in same source')).toBeVisible({
            timeout: 2000,
        })
        await expect(hc.getByText('Document excerpt')).toBeVisible()

        // Academic Paper hover shows Pages label in hover content
        const pageChip = message.getByRole('button', { name: 'Academic Paper', exact: true })
        await pageChip.hover()
        await expect(hc.getByText('Academic paper methodology section')).toBeVisible({
            timeout: 2000,
        })
        await expect(hc.getByText('Pages 3–5')).toBeVisible()

        // Another Doc (exact match — not "Another Document") hover
        const blockChip = message.getByRole('button', { name: 'Another Doc', exact: true })
        await blockChip.hover()
        await expect(hc.getByText('Content block excerpt text')).toBeVisible({ timeout: 2000 })
        await expect(hc.getByText('Document excerpt')).toBeVisible()

        // --- Web search result chip ---
        const webSearchChip = message
            .locator('a[href="https://web-search.example.com/result"]')
            .first()
        await expect(webSearchChip).toBeVisible()
        await expect(webSearchChip).toHaveAttribute('target', '_blank')
        await expect(webSearchChip.evaluate((el) => el.relList?.toString() ?? '')).toMatch(
            /noreferrer/,
        )
        await expect(webSearchChip.evaluate((el) => el.relList?.toString() ?? '')).toMatch(
            /noopener/,
        )
        await webSearchChip.hover()
        await expect(hc.getByText('Web search result excerpt')).toBeVisible({ timeout: 2000 })
        await expect(hc.getByText('Web', { exact: true })).toBeVisible()

        // --- Sources button count ---
        const sourcesButton = message.getByRole('button', { name: /9 sources/i })
        await expect(sourcesButton).toBeVisible()

        // --- Open drawer ---
        await sourcesButton.click()
        const drawer = page.locator('[data-slot="drawer-content"]')
        await expect(drawer).toBeVisible()

        await expect
            .poll(async () => {
                const box = await drawer.evaluate((el) => {
                    const r = el.getBoundingClientRect()
                    return { right: r.right, windowWidth: window.innerWidth }
                })
                return box.right
            })
            .toBeCloseTo(1280, 0)

        const drawerHeight = await drawer.evaluate((el) => el.getBoundingClientRect().height)
        expect(drawerHeight).toBeGreaterThan(300)

        await expect(drawer.getByRole('heading', { name: /sources/i })).toBeVisible()

        // Ordered deduped sources (fixture encounter order)
        const expectedTitles = [
            'Citation source',
            'Bullet citation source',
            'A Very Long Document Title That Should Visually Truncate When Displayed as an Inline Chip in the Chat Interface to Keep the Layout Clean',
            'Web Source Example',
            'Another Document',
            'Google Doc Research',
            'Academic Paper',
            'Web Search Title',
            'Another Doc',
        ]
        const titleElements = drawer.locator('[data-testid="drawer-source-title"]')
        await expect(titleElements).toHaveText(expectedTitles)

        await expect(drawer.getByText('Citation source', { exact: true })).toHaveCount(1)
        await expect(drawer.getByText('Duplicate: Citation source', { exact: true })).toHaveCount(0)

        await expect(titleElements.first()).toHaveTag('p')

        // Location labels: Document excerpt appears twice (char+block), Pages 3–5
        await expect(drawer.getByText('Document excerpt')).toHaveCount(2)
        await expect(drawer.getByText('Pages 3–5')).toHaveCount(1)

        // Scope location labels to their source cards
        const googleDocDrawerCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]').filter({
                hasText: /^Google Doc Research$/,
            }),
        })
        await expect(googleDocDrawerCard.getByText('Document excerpt')).toHaveCount(1)

        const anotherDocDrawerCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]').filter({
                hasText: /^Another Doc$/,
            }),
        })
        await expect(anotherDocDrawerCard.getByText('Document excerpt')).toHaveCount(1)

        const academicDrawerCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]').filter({
                hasText: /^Academic Paper$/,
            }),
        })
        await expect(academicDrawerCard.getByText('Pages 3–5')).toHaveCount(1)

        // Google Doc Research collapsed to one drawer entry
        await expect(drawer.getByText('Google Doc Research')).toHaveCount(1)

        // Scrollable
        const scrollContainer = drawer.locator('.overflow-y-auto')
        await expect(scrollContainer).toBeAttached()
        const scrollable = await scrollContainer.evaluate((el) => {
            return { scrollHeight: el.scrollHeight, clientHeight: el.clientHeight }
        })
        expect(scrollable.scrollHeight).toBeGreaterThan(scrollable.clientHeight)
        await scrollContainer.evaluate((el) => {
            el.scrollTop = 100
        })
        expect(await scrollContainer.evaluate((el) => el.scrollTop)).toBeGreaterThan(0)

        // Web drawer card — anchor with href, target, rel
        const webDrawerCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]', {
                hasText: 'Web Source Example',
            }),
        })
        await expect(webDrawerCard).toHaveTag('a')
        await expect(webDrawerCard).toHaveAttribute('href', 'https://example.com/cited-web-page')
        await expect(webDrawerCard).toHaveAttribute('target', '_blank')
        await expect(webDrawerCard.evaluate((el) => el.relList?.toString() ?? '')).toMatch(
            /noopener/,
        )

        // Non-http drawer source card is a div
        const nonHttpCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]', {
                hasText: 'Bullet citation source',
            }),
        })
        await expect(nonHttpCard).toHaveTag('div')

        // Web Search card — anchor with href, target
        const webSearchCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]').filter({
                hasText: /^Web Search Title$/,
            }),
        })
        await expect(webSearchCard).toHaveTag('a')
        await expect(webSearchCard).toHaveAttribute('href', 'https://web-search.example.com/result')
        await expect(webSearchCard).toHaveAttribute('target', '_blank')
        await expect(webSearchCard.evaluate((el) => el.relList?.toString() ?? '')).toMatch(
            /noreferrer/,
        )
        await expect(webSearchCard.evaluate((el) => el.relList?.toString() ?? '')).toMatch(
            /noopener/,
        )

        // Document drawer card is a div
        const docCard = drawer.locator('[data-testid="drawer-source"]').filter({
            has: page.locator('[data-testid="drawer-source-title"]').filter({
                hasText: /^Google Doc Research$/,
            }),
        })
        await expect(docCard).toHaveTag('div')

        // Close via Escape
        const triggerButtonText = await sourcesButton.textContent()
        await page.keyboard.press('Escape')
        await expect(drawer).not.toBeVisible()
        const focusedAfterEscape = await page.evaluate(() => document.activeElement?.textContent)
        expect(focusedAfterEscape).toContain(triggerButtonText?.trim())

        // Re-open and close via close button
        await page.getByRole('button', { name: /9 sources/i }).click()
        await expect(drawer).toBeVisible()
        await drawer.locator('button').filter({ hasText: /close/i }).click()
        await expect(drawer).not.toBeVisible()
        const focusedAfterCloseBtn = await page.evaluate(() => document.activeElement?.textContent)
        expect(focusedAfterCloseBtn).toContain(triggerButtonText?.trim())

        // Re-open and close via overlay click
        await page.getByRole('button', { name: /9 sources/i }).click()
        await expect(drawer).toBeVisible()
        const overlay = page.locator('[data-slot="drawer-overlay"]')
        await expect(overlay).toBeVisible()
        await overlay.click({ position: { x: 10, y: 10 } })
        await expect(drawer).not.toBeVisible()
        await expect(sourcesButton).toBeFocused()

        // --- Citation-free assistant message has no Sources trigger ---
        const noCitationMessage = page.getByTestId(
            `chat-message-${seeded.citationFreeAssistantMessageId}`,
        )
        await expect(
            noCitationMessage.getByText('Here is an answer with no citations at all.'),
        ).toBeVisible()
        await expect(noCitationMessage.getByRole('button', { name: /source/i })).toHaveCount(0)
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

test('chat reconnects after a dropped stream connection', async ({ page }) => {
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
        await page.route(`**/api/chat/${seeded.chatId}/stream/status`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    active: false,
                    running: false,
                    resumable: false,
                    pendingApproval: false,
                    pendingOAuth: false,
                }),
            })
        })

        let streamRequests = 0
        await page.route(`**/api/chat/${seeded.chatId}/stream`, async (route) => {
            streamRequests += 1
            if (streamRequests === 1) {
                await route.abort('failed')
                return
            }
            await route.fulfill({
                status: 200,
                headers: {
                    'content-type': 'text/event-stream',
                    'cache-control': 'no-cache',
                    connection: 'keep-alive',
                },
                body: `${finalAssistantTextSse('Recovered after reconnecting to the active stream.')}
event: end_of_stream
data: {}

`,
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('Start a stream that drops')
        await page.keyboard.press('Enter')

        await expect(page.getByText('Start a stream that drops')).toBeVisible()
        await expect(
            page.getByText('Recovered after reconnecting to the active stream.'),
        ).toBeVisible({
            timeout: 10_000,
        })
        expect(streamRequests).toBeGreaterThanOrEqual(2)
    } finally {
        await cleanupChat(seeded)
    }
})

test('chat inserts failed tool result before a user reply after interrupted tool call', async ({
    page,
}) => {
    let seeded: (SeededChat & { assistantMessageId: string; toolUseId: string }) | null = null
    try {
        seeded = await seedInterruptedToolCallChat()
        await authenticate(page, seeded)

        await page.route(`**/api/chat/${seeded.chatId}/stream/status`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    active: false,
                    running: false,
                    resumable: false,
                    pendingApproval: false,
                    pendingOAuth: false,
                }),
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
                body: 'event: end_of_stream\ndata: Stream ended\n\n',
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('Please continue after failure')
        await page.keyboard.press('Enter')
        await expect(page.getByText('Please continue after failure')).toBeVisible()

        const sql = postgres(dbConfig)
        const rows = await sql<InterruptedToolRepairRow[]>`
            SELECT id, parent_id, message_seq_num, message
            FROM chat_messages
            WHERE chat_id = ${seeded.chatId}
            ORDER BY message_seq_num
        `
        await sql.end()

        expect(rows).toHaveLength(4)
        const repairMessage = rows[2]
        const followUpMessage = rows[3]
        const repairPayload = repairMessage.message as InterruptedToolResultMessage
        const followUpPayload = followUpMessage.message as TextUserMessage

        expect(repairMessage.parent_id).toBe(seeded.assistantMessageId)
        expect(repairPayload.role).toBe('user')
        expect(repairPayload.content).toEqual([
            expect.objectContaining({
                type: 'tool_result',
                tool_use_id: seeded.toolUseId,
                is_error: true,
            }),
        ])
        expect(repairPayload.content[0].content[0].text).toContain(
            'previous response was interrupted',
        )
        expect(followUpMessage.parent_id).toBe(repairMessage.id)
        expect(followUpPayload).toEqual({
            role: 'user',
            content: 'Please continue after failure',
        })
    } finally {
        await cleanupChat(seeded)
    }
})

test('chat keeps prior messages visible when reloaded during an active stream', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    const fixtureName = `reload-active-stream-${ulid()}.sse`
    const fixturePath = new URL(`./fixtures/${fixtureName}`, import.meta.url)
    try {
        seeded = await seedChat()
        const historicAssistantId = ulid()
        const sql = postgres(dbConfig)
        await sql`
            INSERT INTO chat_messages (id, chat_id, parent_id, message_seq_num, message, content_text)
            VALUES (
                ${historicAssistantId},
                ${seeded.chatId},
                ${seeded.userMessageId},
                2,
                ${sql.json({
                    role: 'assistant',
                    content: [{ type: 'text', text: 'Historical assistant answer before reload.' }],
                })},
                'Historical assistant answer before reload.'
            )
        `
        await sql.end()
        await authenticate(page, seeded)
        await selectReplayFixture(page, fixtureName)

        await writeFile(
            fixturePath,
            `${finalAssistantTextSse('Recovered assistant response after reload.')}${Array.from(
                { length: 500 },
                () => 'event: heartbeat\ndata: {}\n\n',
            ).join('')}event: end_of_stream\ndata: Stream ended\n\n`,
        )

        let streamActive = false
        await page.route(`**/api/chat/${seeded.chatId}/stream/status`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    active: streamActive,
                    running: streamActive,
                    resumable: false,
                    pendingApproval: false,
                    pendingOAuth: false,
                }),
            })
        })

        await page.route(`**/api/chat/${seeded.chatId}/messages`, async (route) => {
            const requestBody = (await route.request().postDataJSON()) as {
                content: string
                parentId?: string
            }
            const messageId = ulid()
            const sql = postgres(dbConfig)
            await sql`
                INSERT INTO chat_messages (id, chat_id, parent_id, message_seq_num, message, content_text)
                VALUES (
                    ${messageId},
                    ${seeded.chatId},
                    ${requestBody.parentId ?? null},
                    3,
                    ${sql.json({ role: 'user', content: requestBody.content })},
                    ${requestBody.content}
                )
            `
            await sql.end()
            streamActive = true
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ messageId }),
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await expect(page.getByText('What tools can you use?')).toBeVisible()
        await expect(page.getByText('Historical assistant answer before reload.')).toBeVisible()

        const textbox = page.getByRole('main').getByRole('textbox')
        await textbox.fill('Reload this stream mid-flight')
        await page.keyboard.press('Enter')
        await expect(page.getByText('Reload this stream mid-flight')).toBeVisible()
        await expect(page.getByText('Recovered assistant response after reload.')).toBeVisible()

        await page.reload({ waitUntil: 'domcontentloaded' })
        await expect(page.getByText('Recovered assistant response after reload.')).toBeVisible()

        await expect(page.getByText('What tools can you use?')).toBeVisible()
        await expect(page.getByText('Historical assistant answer before reload.')).toBeVisible()
        await expect(page.getByText('Reload this stream mid-flight')).toBeVisible()
        await expect(page.locator('.omni-composer-send.rounded-full')).toBeVisible()

        streamActive = false
    } finally {
        await unlink(fixturePath).catch(() => undefined)
        await cleanupChat(seeded)
    }
})

test('approval card can be approved after reload', async ({ page }) => {
    let seeded: SeededChat | null = null
    const fixtureName = `approval-reload-${ulid()}.sse`
    const fixturePath = new URL(`./fixtures/${fixtureName}`, import.meta.url)
    const approvalFixture: ApprovalPauseFixture = {
        approvalId: ulid(),
        toolCallId: `call_${ulid()}`,
        toolName: 'google_drive__google_workspace_call',
        toolInput: {
            service: 'sheets',
            resource: 'spreadsheets.values',
            method: 'update',
            params: { spreadsheetId: 'spreadsheet-1', range: 'Sheet1!A1:B2' },
            body: {
                values: [
                    ['Asset', 'Risk'],
                    ['Debt', 'Low'],
                ],
            },
        },
    }

    try {
        seeded = await seedChatFromTemplateFixture()
        const sql = postgres(dbConfig)
        await sql`
            INSERT INTO tool_approvals (id, chat_id, user_id, tool_name, tool_input)
            VALUES (
                ${approvalFixture.approvalId},
                ${seeded.chatId},
                ${seeded.userId},
                ${approvalFixture.toolName},
                ${sql.json(approvalFixture.toolInput)}
            )
        `
        await sql.end()
        await authenticate(page, seeded)
        await selectReplayFixture(page, fixtureName)

        await writeFile(fixturePath, approvalPauseSse(approvalFixture))

        await page.route(`**/api/chat/${seeded.chatId}/stream/status`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    active: true,
                    running: false,
                    resumable: false,
                    pendingApproval: true,
                    pendingOAuth: false,
                }),
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('Trigger an approval')
        await page.keyboard.press('Enter')

        await expect(page.getByText('Awaiting approval')).toBeVisible()
        await expect(page.getByRole('button', { name: /Approve\s*&\s*send/ })).toBeVisible()

        await writeFile(
            fixturePath,
            [
                approvalRequiredSse(approvalFixture),
                'event: end_of_stream\ndata: Approval required\n\n',
            ].join(''),
        )
        await page.reload({ waitUntil: 'domcontentloaded' })

        await expect(page.getByText('Awaiting approval')).toBeVisible()
        const approveButton = page.getByRole('button', { name: /Approve\s*&\s*send/ })
        await expect(approveButton).toBeVisible()

        await writeFile(
            fixturePath,
            `${finalAssistantTextSse('Approved action completed after reload.')}event: end_of_stream\ndata: {}\n\n`,
        )
        await Promise.all([
            page.waitForResponse(
                (response) =>
                    response.url().includes(`/api/chat/${seeded!.chatId}/approve`) &&
                    response.status() === 200,
            ),
            approveButton.click(),
        ])

        await expect(page.getByText('Awaiting approval')).toHaveCount(0)
    } finally {
        await unlink(fixturePath).catch(() => undefined)
        await cleanupChat(seeded)
    }
})

test('chat reconnects instead of sending a new message while a response is active', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChat()
        await authenticate(page, seeded)

        let statusRequests = 0
        await page.route(`**/api/chat/${seeded.chatId}/stream/status`, async (route) => {
            statusRequests += 1
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    active: statusRequests > 1,
                    running: statusRequests > 1,
                    resumable: false,
                    pendingApproval: false,
                    pendingOAuth: false,
                }),
            })
        })

        await page.route(`**/api/chat/${seeded.chatId}/messages`, async (route) => {
            await route.fulfill({
                status: 409,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({
                    error: 'A response is still in progress for this chat.',
                    streamActive: true,
                }),
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
                body: `${finalAssistantTextSse('Continued the prior response after reconnect.')}
event: end_of_stream
data: {}

`,
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await expect.poll(() => statusRequests).toBe(1)

        const textbox = page.getByRole('main').getByRole('textbox')
        await textbox.fill('Do not send this while the prior response is active')
        await page.keyboard.press('Enter')

        await expect(
            page.getByText('The previous response is still in progress. Reconnecting to it now.'),
        ).toBeVisible()
        await expect(textbox).toContainText('Do not send this while the prior response is active')
        await expect(page.getByText('Continued the prior response after reconnect.')).toBeVisible()
    } finally {
        await cleanupChat(seeded)
    }
})

test('chat renders a sanitized captured stream with incrementally replayed markdown', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChatFromTemplateFixture(capturedSearchesTemplateFixture)
        await authenticate(page, seeded)
        await selectReplayFixture(page, 'captured-searches-stream.sse')

        await page.goto(`/chat/${seeded.chatId}`)
        await page
            .getByRole('main')
            .getByRole('textbox')
            .fill('Replay the synthetic stale markdown stream')
        await page.keyboard.press('Enter')

        await expect(
            page.getByText("The searches aren't returning additional details"),
        ).toBeVisible({ timeout: 30_000 })
        await expect(page.getByText('organizational challenges at SyntheticCo/Acme')).toBeVisible()
    } finally {
        await cleanupChat(seeded)
    }
})

test('chat renders a captured stream from a seeded chat fixture', async ({ page }) => {
    test.skip(
        !capturedSeedChatPath ||
            !capturedExpectedText ||
            !capturedSubmitText ||
            !process.env.OMNI_TEST_CHAT_STREAM_REPLAY_PATH,
        'Set OMNI_CAPTURE_SEED_CHAT_PATH, OMNI_CAPTURE_SUBMIT_TEXT, OMNI_CAPTURE_EXPECT_TEXT, and OMNI_TEST_CHAT_STREAM_REPLAY_PATH to run this capture replay test.',
    )

    let seeded: SeededChat | null = null
    try {
        seeded = await seedChatFromTemplateFixture(capturedSeedChatPath!)
        await authenticate(page, seeded)

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill(capturedSubmitText!)
        await page.keyboard.press('Enter')

        await expect(page.getByText(capturedExpectedText!)).toBeVisible({ timeout: 30_000 })
    } finally {
        await cleanupChat(seeded)
    }
})

test('stopping a partial assistant stream allows a follow-up message to be submitted', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChat()
        await authenticate(page, seeded)
        await selectReplayFixture(page, 'cancel-partial-stream.sse')

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('Start the partial stream')
        await page.keyboard.press('Enter')

        await expect(page.getByText('Start the partial stream')).toBeVisible()
        await expect(page.getByText('Partial answer before stop.')).toBeVisible()

        const stopButton = page.locator('.omni-composer-send.rounded-full')
        await expect(stopButton).toBeVisible()
        await stopButton.click()
        await expect(stopButton).not.toBeVisible()

        const textbox = page.getByRole('main').getByRole('textbox')
        await textbox.fill('Follow-up after stop')
        await page.keyboard.press('Enter')

        await expect
            .poll(() => countMessagesContaining(seeded!.chatId, 'user', 'Follow-up after stop'), {
                message: 'follow-up user message should be persisted after stopping',
            })
            .toBe(1)
    } finally {
        await cleanupChat(seeded)
    }
})

test('stopping a partial assistant stream persists the partial content across reload', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChat()
        await authenticate(page, seeded)
        await selectReplayFixture(page, 'cancel-partial-stream.sse')

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('Start a persisted partial stream')
        await page.keyboard.press('Enter')

        await expect(page.getByText('Partial answer before stop.')).toBeVisible()

        const stopButton = page.locator('.omni-composer-send.rounded-full')
        await expect(stopButton).toBeVisible()
        await stopButton.click()
        await expect(stopButton).not.toBeVisible()

        await expect
            .poll(
                () =>
                    countMessagesContaining(
                        seeded!.chatId,
                        'assistant',
                        'Partial answer before stop.',
                    ),
                { message: 'partial assistant content should be persisted to chat_messages' },
            )
            .toBe(1)

        await page.reload()
        await expect(page.getByText('Partial answer before stop.')).toBeVisible()
    } finally {
        await cleanupChat(seeded)
    }
})

test('stop button during streaming resets state so the input is ready for a new message', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    try {
        seeded = await seedChat()
        await authenticate(page, seeded)

        let messagePosts = 0
        await page.route(`**/api/chat/${seeded.chatId}/messages`, async (route) => {
            messagePosts += 1
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ messageId: ulid() }),
            })
        })

        let resolveFirstStream!: () => void
        const firstStreamUnblocked = new Promise<void>((resolve) => {
            resolveFirstStream = resolve
        })
        let streamRequests = 0
        await page.route(`**/api/chat/${seeded.chatId}/stream`, async (route) => {
            streamRequests += 1
            if (streamRequests === 1) {
                await firstStreamUnblocked
                await route.fulfill({
                    status: 200,
                    headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' },
                    body: 'event: end_of_stream\ndata: Stream stopped\n\n',
                })
                return
            }
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' },
                body: `${finalAssistantTextSse('Follow-up response after stop.')}
event: end_of_stream
data: Stream ended

`,
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('What tools can you use?')
        await page.keyboard.press('Enter')

        // Stop button (rounded-full) appears while the stream is pending
        const stopButton = page.locator('.omni-composer-send.rounded-full')
        await expect(stopButton).toBeVisible()
        await stopButton.click()
        resolveFirstStream()

        // After stopping, the stop button is gone, no empty-response error is shown,
        // and the next message is submitted instead of being treated as an active stream.
        await expect(stopButton).not.toBeVisible()
        await expect(
            page.getByText('Failed to generate response. Please try again.'),
        ).not.toBeVisible()
        const textbox = page.getByRole('main').getByRole('textbox')
        await textbox.fill('Follow-up question')
        await expect(page.locator('.omni-composer-send')).not.toBeDisabled()
        await page.keyboard.press('Enter')

        await expect(page.getByText('Follow-up response after stop.')).toBeVisible()
        expect(messagePosts).toBe(2)
    } finally {
        await cleanupChat(seeded)
    }
})

test('navigating to a different chat while streaming cleans up the stream state', async ({
    page,
}) => {
    let seeded: SeededChat | null = null
    let chat2Id: string | null = null
    const sql = postgres(dbConfig)
    try {
        seeded = await seedChat()
        chat2Id = ulid()
        await sql`
            INSERT INTO chats (id, user_id, title, is_starred, is_deleted)
            VALUES (${chat2Id}, ${seeded.userId}, 'Playwright second chat', false, false)
        `
        await authenticate(page, seeded)

        await page.route(`**/api/chat/${seeded.chatId}/messages`, async (route) => {
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'application/json' },
                body: JSON.stringify({ messageId: ulid() }),
            })
        })

        let resolveStream!: () => void
        const streamUnblocked = new Promise<void>((resolve) => {
            resolveStream = resolve
        })
        await page.route(`**/api/chat/${seeded.chatId}/stream`, async (route) => {
            await streamUnblocked
            await route.fulfill({
                status: 200,
                headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' },
                body: '',
            })
        })

        await page.goto(`/chat/${seeded.chatId}`)
        await page.getByRole('main').getByRole('textbox').fill('What tools can you use?')
        await page.keyboard.press('Enter')

        // Streaming is active on chat 1
        await expect(page.locator('.omni-composer-send.rounded-full')).toBeVisible()

        // Navigate to chat 2 while streaming is in progress
        await page.goto(`/chat/${chat2Id}`)
        resolveStream()

        // Chat 2 must not inherit the streaming state from chat 1
        await expect(page.locator('.omni-composer-send.rounded-full')).not.toBeVisible()
        await expect(page.getByRole('main').getByRole('textbox')).toBeEditable()
    } finally {
        await sql`DELETE FROM chats WHERE id = ${chat2Id}`.catch(() => {})
        await sql.end()
        await cleanupChat(seeded)
    }
})
