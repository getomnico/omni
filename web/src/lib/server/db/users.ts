import { eq, count } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { db } from './index'
import { user } from './schema'
import * as schema from './schema'

export class UserRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = db
    }

    /**
     * Check if any users exist in the system (lightweight check)
     */
    async hasAnyUsers(): Promise<boolean> {
        const [firstUser] = await this.db.select({ id: user.id }).from(user).limit(1)

        return !!firstUser
    }

    /**
     * Check if any admin users exist in the system (lightweight check)
     */
    async hasAnyAdminUsers(): Promise<boolean> {
        const [firstAdmin] = await this.db
            .select({ id: user.id })
            .from(user)
            .where(eq(user.role, 'admin'))
            .limit(1)

        return !!firstAdmin
    }

    /**
     * Get total count of users
     */
    async getUserCount(): Promise<number> {
        const [result] = await this.db.select({ count: count() }).from(user)
        return result.count
    }

    /**
     * Find user by email
     */
    async findByEmail(email: string) {
        const [foundUser] = await this.db
            .select()
            .from(user)
            .where(eq(user.email, email.toLowerCase()))

        return foundUser
    }

    /**
     * Find user by ID
     */
    async findById(id: string) {
        const [foundUser] = await this.db.select().from(user).where(eq(user.id, id))

        return foundUser
    }

    /**
     * Create a new user
     */
    async create(userData: typeof user.$inferInsert) {
        const [newUser] = await this.db.insert(user).values(userData).returning()

        return newUser
    }

    /**
     * Update user by ID
     */
    async update(id: string, userData: Partial<typeof user.$inferInsert>) {
        const [updatedUser] = await this.db
            .update(user)
            .set(userData)
            .where(eq(user.id, id))
            .returning()

        return updatedUser
    }

    /**
     * Check if email exists
     */
    async emailExists(email: string): Promise<boolean> {
        const [existingUser] = await this.db
            .select({ id: user.id })
            .from(user)
            .where(eq(user.email, email.toLowerCase()))
            .limit(1)

        return !!existingUser
    }
}

// Export a default instance for convenience
export const userRepository = new UserRepository()
