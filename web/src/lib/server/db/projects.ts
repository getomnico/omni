import { and, desc, eq, sql } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { ulid } from 'ulid'
import { db } from './index'
import * as schema from './schema'
import { chats, projectAttachments, projects } from './schema'
import type { Project, ProjectAttachment } from './schema'

export type ProjectWithChatCount = Project & { chatCount: number }

export class ProjectNameTakenError extends Error {
    constructor(name: string) {
        super(`A project named "${name}" already exists`)
        this.name = 'ProjectNameTakenError'
    }
}

export class ProjectRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async create(
        userId: string,
        name: string,
        description?: string,
        instructions?: string,
    ): Promise<Project> {
        try {
            const [project] = await this.db
                .insert(projects)
                .values({
                    id: ulid(),
                    userId,
                    name,
                    description: description ?? null,
                    instructions: instructions ?? null,
                })
                .returning()
            return project
        } catch (error) {
            if (isUniqueViolation(error)) {
                throw new ProjectNameTakenError(name)
            }
            throw error
        }
    }

    async get(projectId: string): Promise<Project | null> {
        const [project] = await this.db
            .select()
            .from(projects)
            .where(and(eq(projects.id, projectId), eq(projects.isDeleted, false)))
            .limit(1)
        return project || null
    }

    async getByUserId(userId: string, includeArchived = true): Promise<Project[]> {
        const conditions = [eq(projects.userId, userId), eq(projects.isDeleted, false)]
        if (!includeArchived) {
            conditions.push(eq(projects.isArchived, false))
        }
        return await this.db
            .select()
            .from(projects)
            .where(and(...conditions))
            .orderBy(desc(projects.updatedAt))
    }

    async listWithChatCounts(
        userId: string,
        includeArchived = true,
    ): Promise<ProjectWithChatCount[]> {
        const rows = await this.db
            .select({
                project: projects,
                chatCount: sql<number>`(
                    SELECT count(*)::int
                    FROM "chats" c
                    WHERE c.project_id = projects.id
                      AND c.is_deleted = FALSE
                )`,
            })
            .from(projects)
            .where(
                and(
                    eq(projects.userId, userId),
                    eq(projects.isDeleted, false),
                    ...(includeArchived ? [] : [eq(projects.isArchived, false)]),
                ),
            )
            .orderBy(desc(projects.updatedAt))
        return rows.map((row) => ({ ...row.project, chatCount: row.chatCount }))
    }

    async update(
        projectId: string,
        updates: {
            name?: string
            description?: string | null
            instructions?: string | null
            isArchived?: boolean
        },
    ): Promise<Project | null> {
        try {
            const [updated] = await this.db
                .update(projects)
                .set({ ...updates, updatedAt: new Date() })
                .where(and(eq(projects.id, projectId), eq(projects.isDeleted, false)))
                .returning()
            return updated || null
        } catch (error) {
            if (updates.name !== undefined && isUniqueViolation(error)) {
                throw new ProjectNameTakenError(updates.name)
            }
            throw error
        }
    }

    async delete(projectId: string): Promise<boolean> {
        const deleted = await this.db.transaction(async (tx) => {
            const rows = await tx
                .update(projects)
                .set({ isDeleted: true, updatedAt: new Date() })
                .where(and(eq(projects.id, projectId), eq(projects.isDeleted, false)))
                .returning({ id: projects.id })
            if (rows.length === 0) return false
            // Chats survive project deletion; they return to "unorganized".
            await tx
                .update(chats)
                .set({ projectId: null, updatedAt: new Date() })
                .where(eq(chats.projectId, projectId))
            return true
        })
        return deleted
    }
}

export type ProjectAttachmentType = 'upload' | 'document'

export class ProjectAttachmentRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    async add(
        projectId: string,
        attachment: { type: ProjectAttachmentType; uploadId?: string; documentId?: string },
    ): Promise<ProjectAttachment | null> {
        const values =
            attachment.type === 'upload'
                ? { uploadId: attachment.uploadId ?? null, documentId: null }
                : { documentId: attachment.documentId ?? null, uploadId: null }

        const [row] = await this.db
            .insert(projectAttachments)
            .values({
                id: ulid(),
                projectId,
                attachmentType: attachment.type,
                ...values,
            })
            .onConflictDoNothing()
            .returning()
        return row || null
    }

    async listForProject(projectId: string): Promise<ProjectAttachment[]> {
        return await this.db
            .select()
            .from(projectAttachments)
            .where(eq(projectAttachments.projectId, projectId))
            .orderBy(projectAttachments.addedAt)
    }

    async getOwned(attachmentId: string, userId: string): Promise<ProjectAttachment | null> {
        const [row] = await this.db
            .select({ attachment: projectAttachments })
            .from(projectAttachments)
            .innerJoin(projects, eq(projectAttachments.projectId, projects.id))
            .where(
                and(
                    eq(projectAttachments.id, attachmentId),
                    eq(projects.userId, userId),
                    eq(projects.isDeleted, false),
                ),
            )
            .limit(1)
        return row?.attachment || null
    }

    async remove(attachmentId: string): Promise<boolean> {
        const removed = await this.db
            .delete(projectAttachments)
            .where(eq(projectAttachments.id, attachmentId))
            .returning({ id: projectAttachments.id })
        return removed.length > 0
    }
}

function isUniqueViolation(error: unknown): boolean {
    // Drizzle wraps driver errors; the Postgres code lives on the cause.
    const candidates = [error, (error as { cause?: unknown })?.cause]
    return candidates.some(
        (candidate) =>
            typeof candidate === 'object' &&
            candidate !== null &&
            'code' in candidate &&
            (candidate as { code?: string }).code === '23505',
    )
}
