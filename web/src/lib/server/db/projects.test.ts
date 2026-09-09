import { describe, it, expect, beforeAll, afterAll, beforeEach } from 'vitest'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { eq } from 'drizzle-orm'
import { ulid } from 'ulid'
import { startTestDb, stopTestDb, createTestUser } from './test-setup'
import { ChatRepository } from './chats'
import { ProjectAttachmentRepository, ProjectNameTakenError, ProjectRepository } from './projects'
import * as schema from './schema'

let db: PostgresJsDatabase<typeof schema>
let projectRepo: ProjectRepository
let attachmentRepo: ProjectAttachmentRepository
let chatRepo: ChatRepository
let userId: string

beforeAll(async () => {
    db = await startTestDb()
    projectRepo = new ProjectRepository(db)
    attachmentRepo = new ProjectAttachmentRepository(db)
    chatRepo = new ChatRepository(db)
})

afterAll(async () => {
    await stopTestDb()
})

beforeEach(async () => {
    userId = await createTestUser(db)
})

describe('ProjectRepository CRUD', () => {
    it('creates, reads and lists projects', async () => {
        const created = await projectRepo.create(userId, 'Q3 Launch', 'desc', 'Be terse.')
        expect(created.name).toBe('Q3 Launch')
        expect(created.isArchived).toBe(false)

        const fetched = await projectRepo.get(created.id)
        expect(fetched?.instructions).toBe('Be terse.')

        const list = await projectRepo.getByUserId(userId)
        expect(list.map((p) => p.id)).toEqual([created.id])
    })

    it('enforces per-user unique live names case-insensitively', async () => {
        await projectRepo.create(userId, 'Launch')
        await expect(projectRepo.create(userId, 'launch')).rejects.toThrow(ProjectNameTakenError)

        const otherUserId = await createTestUser(db)
        const other = await projectRepo.create(otherUserId, 'Launch')
        expect(other.name).toBe('Launch')
    })

    it('updates fields and allows clearing nullable ones', async () => {
        const project = await projectRepo.create(userId, 'P1', 'd', 'i')
        const updated = await projectRepo.update(project.id, {
            name: 'P1 renamed',
            description: null,
            isArchived: true,
        })
        expect(updated?.name).toBe('P1 renamed')
        expect(updated?.description).toBeNull()
        expect(updated?.instructions).toBe('i')
        expect(updated?.isArchived).toBe(true)
    })

    it('soft-deletes and detaches chats', async () => {
        const project = await projectRepo.create(userId, 'Doomed')
        const chat = await chatRepo.create(userId, undefined, undefined, undefined, project.id)
        expect(chat.projectId).toBe(project.id)

        expect(await projectRepo.delete(project.id)).toBe(true)

        expect(await projectRepo.get(project.id)).toBeNull()
        const stillThere = await chatRepo.get(chat.id)
        expect(stillThere).not.toBeNull()
        expect(stillThere?.projectId).toBeNull()
    })

    it('listWithChatCounts counts live chats per project', async () => {
        const project = await projectRepo.create(userId, 'Counted')
        const other = await projectRepo.create(userId, 'Empty')
        await chatRepo.create(userId, undefined, undefined, undefined, project.id)
        const deletedChat = await chatRepo.create(
            userId,
            undefined,
            undefined,
            undefined,
            project.id,
        )
        await db
            .update(schema.chats)
            .set({ isDeleted: true })
            .where(eq(schema.chats.id, deletedChat.id))

        const counts = await projectRepo.listWithChatCounts(userId)
        const byId = Object.fromEntries(counts.map((p) => [p.id, p.chatCount]))
        expect(byId[project.id]).toBe(1)
        expect(byId[other.id]).toBe(0)
    })
})

describe('ProjectAttachmentRepository', () => {
    it('adds, lists and removes attachments idempotently', async () => {
        const project = await projectRepo.create(userId, 'With attachments')

        const doc = await attachmentRepo.add(project.id, {
            type: 'document',
            documentId: ulid(),
        })
        expect(doc?.attachmentType).toBe('document')
        expect(doc?.uploadId).toBeNull()

        const duplicate = await attachmentRepo.add(project.id, {
            type: 'document',
            documentId: doc?.documentId ?? undefined,
        })
        expect(duplicate).toBeNull()

        const listed = await attachmentRepo.listForProject(project.id)
        expect(listed.map((a) => a.id)).toEqual([doc?.id])

        expect(await attachmentRepo.remove(doc!.id)).toBe(true)
        expect(await attachmentRepo.remove(doc!.id)).toBe(false)
        expect(await attachmentRepo.listForProject(project.id)).toEqual([])
    })

    it("getOwned only returns attachments of the owner's live projects", async () => {
        const project = await projectRepo.create(userId, 'Owned')
        const doc = await attachmentRepo.add(project.id, {
            type: 'document',
            documentId: ulid(),
        })

        const otherUserId = await createTestUser(db)
        expect(await attachmentRepo.getOwned(doc!.id, otherUserId)).toBeNull()
        expect(await attachmentRepo.getOwned(doc!.id, userId)).not.toBeNull()

        await projectRepo.delete(project.id)
        expect(await attachmentRepo.getOwned(doc!.id, userId)).toBeNull()
    })

    it('cascades attachments when a project is hard-deleted', async () => {
        const project = await projectRepo.create(userId, 'Cascade')
        const doc = await attachmentRepo.add(project.id, {
            type: 'document',
            documentId: ulid(),
        })
        expect(doc).not.toBeNull()
        await db.delete(schema.projects).where(eq(schema.projects.id, project.id))
        expect(await attachmentRepo.listForProject(project.id)).toEqual([])
    })
})

describe('ChatRepository project scoping', () => {
    it('filters history by project', async () => {
        const project = await projectRepo.create(userId, 'Scoped history')
        const inProject = await chatRepo.create(userId, 'Inside', undefined, undefined, project.id)
        await chatRepo.create(userId, 'Outside')

        const scoped = await chatRepo.getByUserId(userId, { projectId: project.id })
        expect(scoped.map((c) => c.id)).toEqual([inProject.id])

        const all = await chatRepo.getByUserId(userId)
        expect(all.length).toBe(2)
    })

    it('moves a chat into and out of a project', async () => {
        const project = await projectRepo.create(userId, 'Move target')
        const chat = await chatRepo.create(userId, 'Movable')

        const moved = await chatRepo.moveChatToProject(chat.id, project.id)
        expect(moved?.projectId).toBe(project.id)

        const removed = await chatRepo.moveChatToProject(chat.id, null)
        expect(removed?.projectId).toBeNull()
    })
})
