import { eq, desc, and } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { db } from './index'
import { chats, chatMessages, user } from './schema'
import type { Chat, ChatMessage } from './schema'
import * as schema from './schema'
import { ulid } from 'ulid'

export class ChatRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    /**
     * Create a new chat
     */
    async create(userId: string, title?: string): Promise<Chat> {
        const chatId = ulid()
        const [newChat] = await this.db
            .insert(chats)
            .values({
                id: chatId,
                userId,
                title,
            })
            .returning()

        return newChat
    }

    /**
     * Get a chat by ID
     */
    async get(chatId: string): Promise<Chat | null> {
        const [chat] = await this.db.select().from(chats).where(eq(chats.id, chatId)).limit(1)

        return chat || null
    }

    /**
     * Get all chats for a user with pagination
     */
    async getByUserId(userId: string, limit?: number, offset?: number): Promise<Chat[]> {
        let query = this.db
            .select()
            .from(chats)
            .where(eq(chats.userId, userId))
            .orderBy(desc(chats.updatedAt))

        if (limit !== undefined) {
            query = query.limit(limit)
        }

        if (offset !== undefined) {
            query = query.offset(offset)
        }

        return await query
    }

    /**
     * Update chat title
     */
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

    /**
     * Delete a chat and all its messages
     */
    async delete(chatId: string): Promise<boolean> {
        const result = await this.db.delete(chats).where(eq(chats.id, chatId))

        return result.rowCount > 0
    }
}

export class ChatMessageRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    /**
     * Create a new chat message
     */
    async create(chatId: string, message: any): Promise<ChatMessage> {
        // Get the next sequence number for this chat
        const nextSeqNum = await this.getNextSequenceNumber(chatId)

        const messageId = ulid()
        const [newMessage] = await this.db
            .insert(chatMessages)
            .values({
                id: messageId,
                chatId,
                messageSeqNum: nextSeqNum,
                message,
            })
            .returning()

        return newMessage
    }

    async update(chatId: string, messageId: string, message: any): Promise<ChatMessage | null> {
        const [updatedMessage] = await this.db
            .update(chatMessages)
            .set({
                message,
            })
            .where(and(eq(chatMessages.id, messageId), eq(chatMessages.chatId, chatId)))
            .returning()

        return updatedMessage || null
    }

    /**
     * Get all messages for a chat, ordered by sequence number
     */
    async getByChatId(chatId: string): Promise<ChatMessage[]> {
        return await this.db
            .select()
            .from(chatMessages)
            .where(eq(chatMessages.chatId, chatId))
            .orderBy(chatMessages.messageSeqNum)
    }

    /**
     * Get the next sequence number for a chat
     */
    private async getNextSequenceNumber(chatId: string): Promise<number> {
        const [lastMessage] = await this.db
            .select({ maxSeq: chatMessages.messageSeqNum })
            .from(chatMessages)
            .where(eq(chatMessages.chatId, chatId))
            .orderBy(desc(chatMessages.messageSeqNum))
            .limit(1)

        return (lastMessage?.maxSeq || 0) + 1
    }

    /**
     * Delete all messages for a chat
     */
    async deleteByChat(chatId: string): Promise<number> {
        const result = await this.db.delete(chatMessages).where(eq(chatMessages.chatId, chatId))

        return result.rowCount
    }
}

// Export default instances for convenience
export const chatRepository = new ChatRepository()
export const chatMessageRepository = new ChatMessageRepository()
