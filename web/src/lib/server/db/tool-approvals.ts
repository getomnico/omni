import { eq, and, asc, desc, inArray } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { db } from './index'
import { toolApprovals } from './schema'
import type { ToolApproval } from './schema'
import * as schema from './schema'
import { ulid } from 'ulid'

type CreateToolApprovalOptions = {
    approvalType?: 'approval' | 'oauth'
    toolCallId?: string | null
    sourceId?: string | null
    sourceType?: string | null
    provider?: string | null
    oauthStartUrl?: string | null
}

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
        options: CreateToolApprovalOptions = {},
    ): Promise<ToolApproval> {
        return this.createWithId(ulid(), chatId, userId, toolName, toolInput, options)
    }

    async createWithId(
        id: string,
        chatId: string,
        userId: string,
        toolName: string,
        toolInput: Record<string, unknown>,
        options: CreateToolApprovalOptions = {},
    ): Promise<ToolApproval> {
        const [approval] = await this.db
            .insert(toolApprovals)
            .values({
                id,
                chatId,
                userId,
                toolName,
                toolInput,
                approvalType: options.approvalType ?? 'approval',
                toolCallId: options.toolCallId ?? null,
                sourceId: options.sourceId ?? null,
                sourceType: options.sourceType ?? null,
                provider: options.provider ?? null,
                oauthStartUrl: options.oauthStartUrl ?? null,
            })
            .onConflictDoNothing()
            .returning()

        if (approval) return approval

        const existingApproval = await this.get(id)
        if (!existingApproval) {
            throw new Error(`Tool approval ${id} was not created and no existing row was found`)
        }

        return existingApproval
    }

    async get(approvalId: string): Promise<ToolApproval | null> {
        const [approval] = await this.db
            .select()
            .from(toolApprovals)
            .where(eq(toolApprovals.id, approvalId))
            .limit(1)

        return approval || null
    }

    async getPendingOAuthForUserAndSource(
        approvalId: string,
        userId: string,
        chatId: string,
        sourceId: string,
        sourceType: string,
        provider: string,
    ): Promise<ToolApproval | null> {
        const [approval] = await this.db
            .select()
            .from(toolApprovals)
            .where(
                and(
                    eq(toolApprovals.id, approvalId),
                    eq(toolApprovals.userId, userId),
                    eq(toolApprovals.chatId, chatId),
                    eq(toolApprovals.approvalType, 'oauth'),
                    eq(toolApprovals.status, 'pending'),
                    eq(toolApprovals.sourceId, sourceId),
                    eq(toolApprovals.sourceType, sourceType),
                    eq(toolApprovals.provider, provider),
                ),
            )
            .limit(1)
        return approval || null
    }

    async approvePendingOAuth(
        approvalId: string,
        userId: string,
        chatId: string,
        sourceId: string,
        sourceType: string,
        provider: string,
    ): Promise<ToolApproval | null> {
        const [approval] = await this.db
            .update(toolApprovals)
            .set({
                status: 'approved',
                resolvedAt: new Date(),
                resolvedBy: userId,
            })
            .where(
                and(
                    eq(toolApprovals.id, approvalId),
                    eq(toolApprovals.userId, userId),
                    eq(toolApprovals.chatId, chatId),
                    eq(toolApprovals.approvalType, 'oauth'),
                    eq(toolApprovals.status, 'pending'),
                    eq(toolApprovals.sourceId, sourceId),
                    eq(toolApprovals.sourceType, sourceType),
                    eq(toolApprovals.provider, provider),
                ),
            )
            .returning()
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

    async resolveMany(
        approvalIds: string[],
        status: 'approved' | 'denied',
        resolvedBy: string,
    ): Promise<ToolApproval[]> {
        const resolved: ToolApproval[] = []
        for (const approvalId of approvalIds) {
            const approval = await this.resolve(approvalId, status, resolvedBy)
            if (approval) resolved.push(approval)
        }
        return resolved
    }

    async getPendingForChat(
        chatId: string,
        approvalType?: 'approval' | 'oauth',
    ): Promise<ToolApproval | null> {
        const filters = [eq(toolApprovals.chatId, chatId), eq(toolApprovals.status, 'pending')]
        if (approvalType) {
            filters.push(eq(toolApprovals.approvalType, approvalType))
        }

        const [approval] = await this.db
            .select()
            .from(toolApprovals)
            .where(and(...filters))
            .orderBy(desc(toolApprovals.createdAt))
            .limit(1)

        return approval || null
    }

    async getPendingForChatAll(
        chatId: string,
        approvalType?: 'approval' | 'oauth',
    ): Promise<ToolApproval[]> {
        return this.getForChatAll(chatId, ['pending'], approvalType)
    }

    async getForChatAll(
        chatId: string,
        statuses: Array<'pending' | 'approved' | 'denied' | 'completed' | 'expired'>,
        approvalType?: 'approval' | 'oauth',
    ): Promise<ToolApproval[]> {
        const filters = [eq(toolApprovals.chatId, chatId), inArray(toolApprovals.status, statuses)]
        if (approvalType) {
            filters.push(eq(toolApprovals.approvalType, approvalType))
        }

        return this.db
            .select()
            .from(toolApprovals)
            .where(and(...filters))
            .orderBy(asc(toolApprovals.createdAt))
    }
}

export const toolApprovalRepository = new ToolApprovalRepository()
