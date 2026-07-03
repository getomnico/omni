import { eq, desc, and, sql } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import type { MessageParam } from '@anthropic-ai/sdk/resources'
import { db } from './index'
import { chats, chatMessages } from './schema'
import type { Chat, ChatMessage } from './schema'
import * as schema from './schema'
import { ulid } from 'ulid'

const DEFAULT_CHAT_SEARCH_LIMIT = 20
const DEFAULT_CHAT_SEARCH_MESSAGE_CANDIDATE_LIMIT = 50
const TEXT_SEARCH_CONFIG = 'simple'
const HEADLINE_START_MARKER = '\u0002'
const HEADLINE_STOP_MARKER = '\u0003'
const TITLE_HEADLINE_OPTIONS = `StartSel=${HEADLINE_START_MARKER}, StopSel=${HEADLINE_STOP_MARKER}, HighlightAll=true`
const SNIPPET_HEADLINE_OPTIONS = `StartSel=${HEADLINE_START_MARKER}, StopSel=${HEADLINE_STOP_MARKER}, MaxFragments=2, MaxWords=24, MinWords=6`

type HighlightPart = { text: string; match: boolean }

export type ChatSearchHit = {
    chat: Chat
    titleParts: HighlightPart[]
    snippet: {
        source: 'title' | 'message'
        messageId: string | null
        parts: HighlightPart[]
    } | null
}

export function highlightPartsFromHeadline(headline: string | null | undefined): HighlightPart[] {
    if (!headline) return []

    const parts: HighlightPart[] = []
    let cursor = 0

    while (cursor < headline.length) {
        const start = headline.indexOf(HEADLINE_START_MARKER, cursor)
        if (start === -1) {
            parts.push({ text: headline.slice(cursor), match: false })
            break
        }

        if (start > cursor) {
            parts.push({ text: headline.slice(cursor, start), match: false })
        }

        const matchStart = start + HEADLINE_START_MARKER.length
        const end = headline.indexOf(HEADLINE_STOP_MARKER, matchStart)
        if (end === -1) {
            parts.push({ text: headline.slice(start), match: false })
            break
        }

        if (end > matchStart) {
            parts.push({ text: headline.slice(matchStart, end), match: true })
        }

        cursor = end + HEADLINE_STOP_MARKER.length
    }

    return parts.filter((part) => part.text.length > 0)
}

function extractContentText(message: MessageParam): string | null {
    if (message.role !== 'user' && message.role !== 'assistant') return null

    if (typeof message.content === 'string') return message.content

    const textParts = message.content
        .filter((block) => block.type === 'text')
        .map((block) => block.text)

    return textParts.length > 0 ? textParts.join('\n') : null
}

type ChatSearchSqlRow = Chat & {
    titleHeadline: string | null
    snippetMessageId: string | null
    snippetSource: 'title' | 'message'
    snippetHeadline: string | null
}

export class ChatRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async create(
        userId: string,
        title?: string,
        modelId?: string,
        agentId?: string,
    ): Promise<Chat> {
        const chatId = ulid()
        const [newChat] = await this.db
            .insert(chats)
            .values({
                id: chatId,
                userId,
                title,
                modelId: modelId || null,
                agentId: agentId || null,
            })
            .returning()

        return newChat
    }

    async get(chatId: string): Promise<Chat | null> {
        const [chat] = await this.db
            .select()
            .from(chats)
            .where(and(eq(chats.id, chatId), eq(chats.isDeleted, false)))
            .limit(1)

        return chat || null
    }

    async getByUserId(
        userId: string,
        options?: { limit?: number; offset?: number; isStarred?: boolean },
    ): Promise<Chat[]> {
        const conditions = [eq(chats.userId, userId), eq(chats.isDeleted, false)]
        if (options?.isStarred !== undefined) {
            conditions.push(eq(chats.isStarred, options.isStarred))
        }

        let query = this.db
            .select()
            .from(chats)
            .where(and(...conditions))
            .orderBy(desc(chats.updatedAt))
            .$dynamic()

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
        const updated = await this.db
            .update(chats)
            .set({ isDeleted: true, updatedAt: new Date() })
            .where(and(eq(chats.id, chatId), eq(chats.isDeleted, false)))
            .returning({ id: chats.id })

        return updated.length > 0
    }

    async search(userId: string, query: string): Promise<ChatSearchHit[]> {
        const results = await this.db.execute<ChatSearchSqlRow>(sql`
            WITH title_matches AS (
                SELECT c.id, c.user_id, c.title, c.is_starred, c.model_id, c.agent_id, c.is_deleted, c.created_at, c.updated_at,
                       preview.message_id, preview.content_text,
                       pdb.score(c.id) AS score, 'title'::text AS source
                FROM chats c
                LEFT JOIN LATERAL (
                    SELECT cm.id AS message_id, cm.content_text
                    FROM chat_messages cm
                    WHERE cm.chat_id = c.id
                      AND cm.content_text IS NOT NULL
                      AND btrim(cm.content_text) <> ''
                    ORDER BY cm.message_seq_num ASC
                    LIMIT 1
                ) preview ON TRUE
                WHERE c.title ||| ${query}
                  AND c.user_id = ${userId}
                  AND c.is_deleted = FALSE
                ORDER BY score DESC
                LIMIT ${DEFAULT_CHAT_SEARCH_LIMIT}
            ),
            top_message_matches AS (
                SELECT cm.id AS message_id, cm.chat_id, cm.content_text, pdb.score(cm.id) AS score
                FROM chat_messages cm
                JOIN chats c ON c.id = cm.chat_id
                WHERE cm.content_text ||| ${query}
                  AND c.user_id = ${userId}
                  AND c.is_deleted = FALSE
                ORDER BY score DESC
                LIMIT ${DEFAULT_CHAT_SEARCH_MESSAGE_CANDIDATE_LIMIT}
            ),
            message_matches AS (
                SELECT DISTINCT ON (c.id)
                       c.id, c.user_id, c.title, c.is_starred, c.model_id, c.agent_id, c.is_deleted, c.created_at, c.updated_at,
                       tmm.message_id, tmm.content_text, tmm.score, 'message'::text AS source
                FROM top_message_matches tmm
                JOIN chats c ON c.id = tmm.chat_id
                ORDER BY c.id, tmm.score DESC
            ),
            ranked_matches AS (
                SELECT DISTINCT ON (id)
                       id, user_id, title, is_starred, model_id, agent_id, is_deleted, created_at, updated_at,
                       message_id, content_text, score, source
                FROM (
                    SELECT * FROM title_matches
                    UNION ALL
                    SELECT * FROM message_matches
                ) AS all_matches
                ORDER BY id, score DESC
            ),
            final_candidates AS (
                SELECT *
                FROM ranked_matches
                ORDER BY score DESC
                LIMIT ${DEFAULT_CHAT_SEARCH_LIMIT}
            )
            SELECT id,
                   user_id AS "userId",
                   title,
                   is_starred AS "isStarred",
                   model_id AS "modelId",
                   agent_id AS "agentId",
                   is_deleted AS "isDeleted",
                   created_at AS "createdAt",
                   updated_at AS "updatedAt",
                   ts_headline(
                       ${TEXT_SEARCH_CONFIG}::regconfig,
                       COALESCE(title, ''),
                       plainto_tsquery(${TEXT_SEARCH_CONFIG}::regconfig, ${query}),
                       ${TITLE_HEADLINE_OPTIONS}
                   ) AS "titleHeadline",
                   message_id AS "snippetMessageId",
                   source AS "snippetSource",
                   CASE
                       WHEN source = 'message' THEN ts_headline(
                           ${TEXT_SEARCH_CONFIG}::regconfig,
                           COALESCE(content_text, ''),
                           plainto_tsquery(${TEXT_SEARCH_CONFIG}::regconfig, ${query}),
                           ${SNIPPET_HEADLINE_OPTIONS}
                       )
                       ELSE NULLIF(
                           left(regexp_replace(COALESCE(content_text, ''), '[[:space:]]+', ' ', 'g'), 180),
                           ''
                       )
                   END AS "snippetHeadline"
            FROM final_candidates
            ORDER BY score DESC
        `)

        return results.map((row) => {
            const { titleHeadline, snippetMessageId, snippetSource, snippetHeadline, ...chat } = row
            const titleParts = highlightPartsFromHeadline(titleHeadline)
            const snippetParts = highlightPartsFromHeadline(snippetHeadline)
            const hasHighlightedSnippetMatch = snippetParts.some((part) => part.match)
            const hasSnippetText = snippetParts.some((part) => part.text.trim().length > 0)

            return {
                chat,
                titleParts:
                    titleParts.length > 0
                        ? titleParts
                        : [{ text: row.title ?? 'Untitled', match: false }],
                snippet:
                    (snippetSource === 'message' && hasHighlightedSnippetMatch) ||
                    (snippetSource === 'title' && hasSnippetText)
                        ? {
                              source: snippetSource,
                              messageId: snippetMessageId,
                              parts: snippetParts,
                          }
                        : null,
            }
        })
    }
}

export class ChatMessageRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async create(chatId: string, message: MessageParam, parentId?: string): Promise<ChatMessage> {
        const nextSeqNum = await this.getNextSequenceNumber(chatId)
        const contentText = extractContentText(message)

        const messageId = ulid()
        const [newMessage] = await this.db
            .insert(chatMessages)
            .values({
                id: messageId,
                chatId,
                parentId: parentId || null,
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

    async getByIdInChat(chatId: string, messageId: string): Promise<ChatMessage | null> {
        const [message] = await this.db
            .select()
            .from(chatMessages)
            .where(and(eq(chatMessages.chatId, chatId), eq(chatMessages.id, messageId)))
            .limit(1)

        return message || null
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

    async getActivePath(chatId: string): Promise<ChatMessage[]> {
        const rows = await this.db.execute<ChatMessage>(sql`
            WITH RECURSIVE walk_up AS (
                SELECT cm.id, cm.chat_id, cm.parent_id, cm.message_seq_num, cm.message, cm.content_text, cm.created_at
                FROM (
                    SELECT *
                    FROM chat_messages
                    WHERE chat_id = ${chatId}
                    AND id NOT IN (
                        SELECT DISTINCT parent_id FROM chat_messages
                        WHERE chat_id = ${chatId} AND parent_id IS NOT NULL
                    )
                    ORDER BY message_seq_num DESC
                    LIMIT 1
                ) cm

                UNION ALL

                SELECT cm.id, cm.chat_id, cm.parent_id, cm.message_seq_num, cm.message, cm.content_text, cm.created_at
                FROM chat_messages cm
                JOIN walk_up wu ON cm.id = wu.parent_id
            )
            SELECT id,
                   chat_id AS "chatId",
                   parent_id AS "parentId",
                   message_seq_num AS "messageSeqNum",
                   message,
                   content_text AS "contentText",
                   created_at AS "createdAt"
            FROM walk_up
            ORDER BY message_seq_num
        `)

        return [...rows]
    }

    async getLastMessageInActivePath(chatId: string): Promise<ChatMessage | null> {
        const path = await this.getActivePath(chatId)
        return path.length > 0 ? path[path.length - 1] : null
    }

    async deleteByChat(chatId: string): Promise<number> {
        const deleted = await this.db
            .delete(chatMessages)
            .where(eq(chatMessages.chatId, chatId))
            .returning({ id: chatMessages.id })

        return deleted.length
    }
}

export const chatRepository = new ChatRepository()
export const chatMessageRepository = new ChatMessageRepository()
