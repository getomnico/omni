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
