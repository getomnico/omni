import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import type { MessageParam } from '@anthropic-ai/sdk/resources/messages.js'
import { eq, sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { startTestDb, stopTestDb, createTestUser, createTestChat } from './test-setup'
import { ChatMessageRepository, ChatRepository, highlightPartsFromHeadline } from './chats'
import * as schema from './schema'

let db: PostgresJsDatabase<typeof schema>
let repo: ChatMessageRepository
let chatRepo: ChatRepository
let userId: string
let chatId: string

function userMsg(text: string): MessageParam {
    return { role: 'user', content: text }
}

function assistantMsg(text: string): MessageParam {
    return { role: 'assistant', content: text }
}

beforeAll(async () => {
    db = await startTestDb()
    repo = new ChatMessageRepository(db)
    chatRepo = new ChatRepository(db)
})

afterAll(async () => {
    await stopTestDb()
})

beforeEach(async () => {
    userId = await createTestUser(db)
    chatId = await createTestChat(db, userId)
})

describe('ChatMessageRepository branching', () => {
    it('getActivePath returns empty array for chat with no messages', async () => {
        const path = await repo.getActivePath(chatId)
        expect(path).toEqual([])
    })

    it('getActivePath returns single message for root-only chat', async () => {
        const root = await repo.create(chatId, userMsg('hello'))
        const path = await repo.getActivePath(chatId)
        expect(path.map((m) => m.id)).toEqual([root.id])
    })

    it('getActivePath returns linear chain in order', async () => {
        const root = await repo.create(chatId, userMsg('hello'))
        const a = await repo.create(chatId, assistantMsg('hi'), root.id)
        const b = await repo.create(chatId, userMsg('how are you?'), a.id)
        const c = await repo.create(chatId, assistantMsg('good!'), b.id)

        const path = await repo.getActivePath(chatId)

        expect(path.map((m) => m.id)).toEqual([root.id, a.id, b.id, c.id])
    })

    it('getActivePath returns path to highest seq leaf in branched tree', async () => {
        // root(1) -> A(2) -> B(3) -> C(4)
        //                 -> B'(5) -> C'(6)
        const root = await repo.create(chatId, userMsg('hello'))
        const a = await repo.create(chatId, assistantMsg('hi'), root.id)
        const b = await repo.create(chatId, userMsg('option 1'), a.id)
        await repo.create(chatId, assistantMsg('response 1'), b.id)
        const bPrime = await repo.create(chatId, userMsg('option 2'), a.id)
        const cPrime = await repo.create(chatId, assistantMsg('response 2'), bPrime.id)

        const path = await repo.getActivePath(chatId)

        expect(path.map((m) => m.id)).toEqual([root.id, a.id, bPrime.id, cPrime.id])
    })

    it('adding to non-active branch shifts active path', async () => {
        const root = await repo.create(chatId, userMsg('hello'))
        const a = await repo.create(chatId, assistantMsg('hi'), root.id)
        const b = await repo.create(chatId, userMsg('option 1'), a.id)
        const c = await repo.create(chatId, assistantMsg('response 1'), b.id)
        const bPrime = await repo.create(chatId, userMsg('option 2'), a.id)
        await repo.create(chatId, assistantMsg('response 2'), bPrime.id)

        // Add D as child of C (the non-active branch) — this should shift the active path
        const d = await repo.create(chatId, userMsg('follow up'), c.id)

        const path = await repo.getActivePath(chatId)

        expect(path.map((m) => m.id)).toEqual([root.id, a.id, b.id, c.id, d.id])
    })

    it('edit creates sibling and active path follows new branch', async () => {
        const root = await repo.create(chatId, userMsg('hello'))
        const a = await repo.create(chatId, assistantMsg('hi'), root.id)
        const b = await repo.create(chatId, userMsg('original'), a.id)

        // Simulate edit: create B' with same parent as B (i.e., A)
        const bPrime = await repo.create(chatId, userMsg('edited'), a.id)

        expect(b.parentId).toBe(a.id)
        expect(bPrime.parentId).toBe(a.id)

        const path = await repo.getActivePath(chatId)

        expect(path.map((m) => m.id)).toEqual([root.id, a.id, bPrime.id])
    })
})

describe('ChatRepository history and search', () => {
    it('getByUserId() paginates full history and includes starred chats unless filtered', async () => {
        const older = chatId
        const starred = await createTestChat(db, userId, 'starred middle')
        const newest = await createTestChat(db, userId, 'newest chat')

        await db
            .update(schema.chats)
            .set({ updatedAt: new Date('2026-01-01T00:00:00.000Z') })
            .where(eq(schema.chats.id, older))
        await db
            .update(schema.chats)
            .set({ updatedAt: new Date('2026-01-02T00:00:00.000Z'), isStarred: true })
            .where(eq(schema.chats.id, starred))
        await db
            .update(schema.chats)
            .set({ updatedAt: new Date('2026-01-03T00:00:00.000Z') })
            .where(eq(schema.chats.id, newest))

        const secondPage = await chatRepo.getByUserId(userId, { limit: 2, offset: 1 })
        expect(secondPage.map((chat) => chat.id)).toEqual([starred, older])

        const unstarred = await chatRepo.getByUserId(userId, { isStarred: false })
        expect(unstarred.map((chat) => chat.id)).toContain(newest)
        expect(unstarred.map((chat) => chat.id)).toContain(older)
        expect(unstarred.map((chat) => chat.id)).not.toContain(starred)
    })

    it('search() includes starred chats', async () => {
        const starred = await createTestChat(db, userId, 'Unique Narwhal Roadmap')
        await db.update(schema.chats).set({ isStarred: true }).where(eq(schema.chats.id, starred))

        const results = await chatRepo.search(userId, 'narwhal')

        expect(results.map((chat) => chat.id)).toContain(starred)
        expect(results.find((chat) => chat.id === starred)?.isStarred).toBe(true)
    })

    it('search() returns the best matching message snippet with safe highlight parts', async () => {
        const searchChat = await createTestChat(db, userId, 'Kitchen planning')
        await repo.create(searchChat, userMsg('This mentions dragonfruit once.'))
        const bestMessage = await repo.create(
            searchChat,
            assistantMsg('dragonfruit dragonfruit dragonfruit shows the best matching context.'),
        )

        const results = await chatRepo.search(userId, 'dragonfruit')
        const result = results.find((chat) => chat.id === searchChat)

        expect(result).toBeDefined()
        expect(result?.snippet?.source).toBe('message')
        expect(result?.snippet?.messageId).toBe(bestMessage.id)
        expect(
            result?.snippet?.parts.some((part) => part.match && /dragonfruit/i.test(part.text)),
        ).toBe(true)
    })

    it('highlightPartsFromHeadline() converts marker output to structured non-HTML parts', () => {
        expect(highlightPartsFromHeadline('alpha **bravo** <script>charlie</script>')).toEqual([
            { text: 'alpha ', match: false },
            { text: 'bravo', match: true },
            { text: ' <script>charlie</script>', match: false },
        ])
    })
})

describe('ChatRepository soft-delete', () => {
    it('delete() flips is_deleted instead of removing the row', async () => {
        const ok = await chatRepo.delete(chatId)
        expect(ok).toBe(true)

        const [row] = await db
            .select({ id: schema.chats.id, isDeleted: schema.chats.isDeleted })
            .from(schema.chats)
            .where(eq(schema.chats.id, chatId))

        expect(row).toBeDefined()
        expect(row.isDeleted).toBe(true)
    })

    it('delete() preserves linked model_usage rows', async () => {
        // Insert a model + model_usage row tied to this chat, then soft-delete.
        const modelId = await createTestModel(db)
        await db.execute(sql`
            INSERT INTO model_usage (id, user_id, model_id, model_name, provider_type, purpose, chat_id, input_tokens, output_tokens)
            VALUES (${ulid()}, ${userId}, ${modelId}, 'test-model', 'test', 'chat', ${chatId}, 10, 20)
        `)

        await chatRepo.delete(chatId)

        const result = await db.execute(
            sql`SELECT count(*)::int AS n FROM model_usage WHERE chat_id = ${chatId}`,
        )
        expect((result[0] as { n: number }).n).toBe(1)
    })

    it('get() returns null for a soft-deleted chat', async () => {
        await chatRepo.delete(chatId)
        const chat = await chatRepo.get(chatId)
        expect(chat).toBeNull()
    })

    it('getByUserId() excludes soft-deleted chats', async () => {
        const otherChatId = await createTestChat(db, userId, 'kept')
        await chatRepo.delete(chatId)

        const chats = await chatRepo.getByUserId(userId)
        expect(chats.map((c) => c.id)).toEqual([otherChatId])
    })

    it('delete() returns false for an already-deleted chat', async () => {
        expect(await chatRepo.delete(chatId)).toBe(true)
        expect(await chatRepo.delete(chatId)).toBe(false)
    })
})

async function createTestModel(database: PostgresJsDatabase<typeof schema>): Promise<string> {
    const providerId = ulid()
    const modelId = ulid()
    await database.insert(schema.modelProviders).values({
        id: providerId,
        name: 'test-provider',
        providerType: 'anthropic',
    })
    await database.insert(schema.models).values({
        id: modelId,
        modelProviderId: providerId,
        modelId: 'test-model',
        displayName: 'test-model',
    })
    return modelId
}
