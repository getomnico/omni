import { eq, and, or, desc, sql } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { db } from './index'
import { skills } from './schema'
import type { Skill } from './schema'
import * as schema from './schema'
import type { SkillVisibility } from '$lib/skills.js'
import { ulid } from 'ulid'

export class SkillRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    /**
     * List skills visible to the given user: public skills + user's own private skills.
     */
    async listVisible(userId: string): Promise<Skill[]> {
        return this.db
            .select()
            .from(skills)
            .where(or(eq(skills.ownerId, userId), eq(skills.visibility, 'public')))
            .orderBy(desc(skills.updatedAt))
    }

    /**
     * Get a single skill by ID if it is visible to the given user.
     * Returns null if the skill does not exist or is not visible.
     */
    async getVisibleById(id: string, userId: string): Promise<Skill | null> {
        const [row] = await this.db
            .select()
            .from(skills)
            .where(
                and(
                    eq(skills.id, id),
                    or(eq(skills.ownerId, userId), eq(skills.visibility, 'public')),
                ),
            )
            .limit(1)
        return row || null
    }

    /**
     * Create a new skill owned by the given user.
     */
    async create(data: {
        userId: string
        name: string
        instructions: string
        visibility?: SkillVisibility
    }): Promise<Skill> {
        const id = ulid()
        const [row] = await this.db
            .insert(skills)
            .values({
                id,
                ownerId: data.userId,
                name: data.name,
                instructions: data.instructions,
                visibility: data.visibility || 'private',
            })
            .returning()
        return row
    }

    /**
     * Update a skill if the user is the owner.
     * Returns the updated skill, or null if not found or not owned.
     */
    async update(
        id: string,
        userId: string,
        data: Partial<{
            name: string
            instructions: string
            visibility: SkillVisibility
        }>,
    ): Promise<Skill | null> {
        const [row] = await this.db
            .update(skills)
            .set(data)
            .where(and(eq(skills.id, id), eq(skills.ownerId, userId)))
            .returning()
        return row || null
    }

    /**
     * Delete a skill if the user is the owner.
     * Returns the deleted skill, or null if not found or not owned.
     */
    async delete(id: string, userId: string): Promise<Skill | null> {
        const [row] = await this.db
            .delete(skills)
            .where(and(eq(skills.id, id), eq(skills.ownerId, userId)))
            .returning()
        return row || null
    }

    /**
     * Clone a public skill into a new private skill owned by the target user.
     * Returns the new skill, or null if the source is not visible or not public.
     */
    async clone(sourceId: string, newOwnerId: string): Promise<Skill | null> {
        const id = ulid()
        const rows = await this.db.execute<Skill>(sql`
            INSERT INTO skills (id, owner_id, name, instructions, visibility)
            SELECT ${id}, ${newOwnerId}, name, instructions, 'private'
            FROM skills
            WHERE id = ${sourceId}
              AND visibility = 'public'
            RETURNING
                id,
                owner_id AS "ownerId",
                name,
                instructions,
                visibility,
                created_at AS "createdAt",
                updated_at AS "updatedAt"
        `)
        return rows[0] || null
    }
}
