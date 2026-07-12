import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { eq } from 'drizzle-orm'
import { ulid } from 'ulid'
import { startTestDb, stopTestDb, createTestUser, createTestChat } from './test-setup'
import { ToolApprovalRepository } from './tool-approvals'
import * as schema from './schema'

let db: PostgresJsDatabase<typeof schema>
let repo: ToolApprovalRepository
let userId: string
let chatId: string

beforeAll(async () => {
    db = await startTestDb()
    repo = new ToolApprovalRepository(db)
})

afterAll(async () => {
    await stopTestDb()
})

beforeEach(async () => {
    userId = await createTestUser(db)
    chatId = await createTestChat(db, userId)
})

describe('ToolApprovalRepository', () => {
    it('finds only the exact pending OAuth approval correlation', async () => {
        const approvalId = ulid()
        const sourceId = ulid()
        const sourceType = 'gmail'
        const provider = 'google'
        const approval = await repo.createWithId(
            approvalId,
            chatId,
            userId,
            'gmail__send_email',
            { to: 'person@example.com' },
            {
                approvalType: 'oauth',
                toolCallId: 'toolu_oauth',
                sourceId,
                sourceType,
                provider,
                oauthStartUrl: `/api/oauth/start?source_id=${sourceId}`,
            },
        )

        await expect(
            repo.getPendingOAuthForUserAndSource(
                approvalId,
                userId,
                chatId,
                sourceId,
                sourceType,
                provider,
            ),
        ).resolves.toEqual(approval)

        const otherUserId = await createTestUser(db)
        const otherChatId = await createTestChat(db, userId)
        const mismatches: Parameters<ToolApprovalRepository['getPendingOAuthForUserAndSource']>[] =
            [
                [approvalId, otherUserId, chatId, sourceId, sourceType, provider],
                [approvalId, userId, otherChatId, sourceId, sourceType, provider],
                [approvalId, userId, chatId, ulid(), sourceType, provider],
                [approvalId, userId, chatId, sourceId, 'google_drive', provider],
                [approvalId, userId, chatId, sourceId, sourceType, 'microsoft'],
            ]
        for (const args of mismatches) {
            await expect(repo.getPendingOAuthForUserAndSource(...args)).resolves.toBeNull()
        }

        const approved = await repo.approvePendingOAuth(
            approvalId,
            userId,
            chatId,
            sourceId,
            sourceType,
            provider,
        )
        expect(approved?.status).toBe('approved')
        await expect(
            repo.getPendingOAuthForUserAndSource(
                approvalId,
                userId,
                chatId,
                sourceId,
                sourceType,
                provider,
            ),
        ).resolves.toBeNull()
    })

    it('createWithId is idempotent for replayed approval_required events', async () => {
        const approvalId = ulid()
        const toolInput = {
            service: 'sheets',
            resource: 'spreadsheets.values',
            method: 'get',
            params: { spreadsheetId: 'spreadsheet-1', range: 'Sheet1!A1:B2' },
        }

        const firstApproval = await repo.createWithId(
            approvalId,
            chatId,
            userId,
            'google_drive__google_workspace_call',
            toolInput,
        )
        const replayedApproval = await repo.createWithId(
            approvalId,
            chatId,
            userId,
            'google_drive__google_workspace_call',
            toolInput,
        )

        expect(replayedApproval).toEqual(firstApproval)

        const approvals = await db
            .select()
            .from(schema.toolApprovals)
            .where(eq(schema.toolApprovals.id, approvalId))
        expect(approvals).toHaveLength(1)
    })
})
