import { eq, desc, and, sql } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import type { MessageParam } from '@anthropic-ai/sdk/resources'
import { db } from './index'
import { chats, chatMessages } from './schema'
import type { Chat, ChatMessage } from './schema'
import * as schema from './schema'
import { ulid } from 'ulid'

function extractContentText(message: MessageParam): string | null {
    if (message.role !== 'user' && message.role !== 'assistant') return null

    if (typeof message.content === 'string') return message.content

    const textParts = message.content
        .filter((block) => block.type === 'text')
        .map((block) => block.text)

    return textParts.length > 0 ? textParts.join('\n') : null
}

export class ChatRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async create(userId: string, title?: string, modelId?: string): Promise<Chat> {
        const chatId = ulid()
        const [newChat] = await this.db
            .insert(chats)
            .values({
                id: chatId,
                userId,
                title,
                modelId: modelId || null,
            })
            .returning()

        return newChat
    }

    async get(chatId: string): Promise<Chat | null> {
        const [chat] = await this.db.select().from(chats).where(eq(chats.id, chatId)).limit(1)

        return chat || null
    }

    async getByUserId(
        userId: string,
        options?: { limit?: number; offset?: number; isStarred?: boolean },
    ): Promise<Chat[]> {
        const conditions = [eq(chats.userId, userId)]
        if (options?.isStarred !== undefined) {
            conditions.push(eq(chats.isStarred, options.isStarred))
        }

        let query = this.db
            .select()
            .from(chats)
            .where(and(...conditions))
            .orderBy(desc(chats.updatedAt))

        if (options?.limit !== undefined) {
            query = query.limit(options.limit)
        }

        if (options?.offset !== undefined) {
            query = query.offset(options.offset)
        }

        return await query
    }

    async updateTitle(chatId: string, title: string): Promise<Chat | null> {
        const [updatedChat] = await this.db
            .update(chats)
            .set({
                title,
                updatedAt: new Date(),
            })
            .where(eq(chats.id, chatId))
            .returning()

        return updatedChat || null
    }

    async toggleStar(chatId: string, isStarred: boolean): Promise<Chat | null> {
        const [updatedChat] = await this.db
            .update(chats)
            .set({
                isStarred,
                updatedAt: new Date(),
            })
            .where(eq(chats.id, chatId))
            .returning()

        return updatedChat || null
    }

    async delete(chatId: string): Promise<boolean> {
        const result = await this.db.delete(chats).where(eq(chats.id, chatId))

        return result.rowCount > 0
    }

    async search(userId: string, query: string): Promise<Chat[]> {
        const results = await this.db.execute(sql`
            WITH title_matches AS (
                SELECT c.id, c.user_id, c.title, c.is_starred, c.model_id, c.created_at, c.updated_at,
                       pdb.score(c.id) AS score
                FROM chats c
                WHERE c.title ||| ${query}
                  AND c.user_id = ${userId}
                ORDER BY score DESC
                LIMIT 20
            ),
            top_message_matches AS (
                SELECT cm.id AS message_id, cm.chat_id, pdb.score(cm.id) AS score
                FROM chat_messages cm
                JOIN chats c ON c.id = cm.chat_id
                WHERE cm.content_text ||| ${query}
                  AND c.user_id = ${userId}
                ORDER BY score DESC
                LIMIT 50
            ),
            message_matches AS (
                SELECT DISTINCT ON (c.id)
                       c.id, c.user_id, c.title, c.is_starred, c.model_id, c.created_at, c.updated_at,
                       tmm.score
                FROM top_message_matches tmm
                JOIN chats c ON c.id = tmm.chat_id
                ORDER BY c.id, tmm.score DESC
            ),
            combined AS (
                SELECT id, user_id, title, is_starred, model_id, created_at, updated_at,
                       MAX(score) AS max_score
                FROM (
                    SELECT * FROM title_matches
                    UNION ALL
                    SELECT * FROM message_matches
                ) AS all_matches
                GROUP BY id, user_id, title, is_starred, model_id, created_at, updated_at
            )
            SELECT id, user_id, title, is_starred, model_id, created_at, updated_at
            FROM combined
            ORDER BY max_score DESC
            LIMIT 20
        `)

        return results.map((row: any) => ({
            id: row.id,
            userId: row.user_id,
            title: row.title,
            isStarred: row.is_starred,
            modelId: row.model_id,
            createdAt: row.created_at,
            updatedAt: row.updated_at,
        }))
    }
}

export class ChatMessageRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async create(chatId: string, message: MessageParam): Promise<ChatMessage> {
        const nextSeqNum = await this.getNextSequenceNumber(chatId)
        const contentText = extractContentText(message)

        const messageId = ulid()
        const [newMessage] = await this.db
            .insert(chatMessages)
            .values({
                id: messageId,
                chatId,
                messageSeqNum: nextSeqNum,
                message,
                contentText,
            })
            .returning()

        return newMessage
    }

    async update(
        chatId: string,
        messageId: string,
        message: MessageParam,
    ): Promise<ChatMessage | null> {
        const contentText = extractContentText(message)
        const [updatedMessage] = await this.db
            .update(chatMessages)
            .set({
                message,
                contentText,
            })
            .where(and(eq(chatMessages.id, messageId), eq(chatMessages.chatId, chatId)))
            .returning()

        return updatedMessage || null
    }

    async getByChatId(chatId: string): Promise<ChatMessage[]> {
        return await this.db
            .select()
            .from(chatMessages)
            .where(eq(chatMessages.chatId, chatId))
            .orderBy(chatMessages.messageSeqNum)
    }

    private async getNextSequenceNumber(chatId: string): Promise<number> {
        const [lastMessage] = await this.db
            .select({ maxSeq: chatMessages.messageSeqNum })
            .from(chatMessages)
            .where(eq(chatMessages.chatId, chatId))
            .orderBy(desc(chatMessages.messageSeqNum))
            .limit(1)

        return (lastMessage?.maxSeq || 0) + 1
    }

    async deleteByChat(chatId: string): Promise<number> {
        const result = await this.db.delete(chatMessages).where(eq(chatMessages.chatId, chatId))

        return result.rowCount
    }
}

export const chatRepository = new ChatRepository()
export const chatMessageRepository = new ChatMessageRepository()
