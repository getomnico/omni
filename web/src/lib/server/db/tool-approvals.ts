import { eq, and, desc } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { db } from './index'
import { toolApprovals } from './schema'
import type { ToolApproval } from './schema'
import * as schema from './schema'
import { ulid } from 'ulid'

export class ToolApprovalRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async create(
        chatId: string,
        userId: string,
        toolName: string,
        toolInput: Record<string, unknown>,
        sourceId?: string,
        sourceType?: string,
    ): Promise<ToolApproval> {
        return this.createWithId(ulid(), chatId, userId, toolName, toolInput, sourceId, sourceType)
    }

    async createWithId(
        id: string,
        chatId: string,
        userId: string,
        toolName: string,
        toolInput: Record<string, unknown>,
        sourceId?: string,
        sourceType?: string,
    ): Promise<ToolApproval> {
        const [approval] = await this.db
            .insert(toolApprovals)
            .values({
                id,
                chatId,
                userId,
                toolName,
                toolInput,
                sourceId: sourceId || null,
                sourceType: sourceType || null,
            })
            .returning()

        return approval
    }

    async get(approvalId: string): Promise<ToolApproval | null> {
        const [approval] = await this.db
            .select()
            .from(toolApprovals)
            .where(eq(toolApprovals.id, approvalId))
            .limit(1)

        return approval || null
    }

    async resolve(
        approvalId: string,
        status: 'approved' | 'denied',
        resolvedBy: string,
    ): Promise<ToolApproval | null> {
        const [approval] = await this.db
            .update(toolApprovals)
            .set({
                status,
                resolvedAt: new Date(),
                resolvedBy,
            })
            .where(eq(toolApprovals.id, approvalId))
            .returning()

        return approval || null
    }

    async getPendingForChat(chatId: string): Promise<ToolApproval | null> {
        const [approval] = await this.db
            .select()
            .from(toolApprovals)
            .where(and(eq(toolApprovals.chatId, chatId), eq(toolApprovals.status, 'pending')))
            .orderBy(desc(toolApprovals.createdAt))
            .limit(1)

        return approval || null
    }
}

export const toolApprovalRepository = new ToolApprovalRepository()
