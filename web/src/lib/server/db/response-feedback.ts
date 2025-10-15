import { eq, and } from 'drizzle-orm'
import type { PostgresJsDatabase } from 'drizzle-orm/postgres-js'
import { db } from './index'
import { responseFeedback } from './schema'
import type { ResponseFeedback } from './schema'
import * as schema from './schema'
import { ulid } from 'ulid'

export type FeedbackType = 'upvote' | 'downvote'

export class ResponseFeedbackRepository {
    private db: PostgresJsDatabase<typeof schema>

    constructor(private dbInstance: PostgresJsDatabase<typeof schema> = db) {
        this.db = dbInstance
    }

    /**
     * Create or update feedback for a message
     */
    async createOrUpdate(
        messageId: string,
        userId: string,
        feedbackType: FeedbackType,
    ): Promise<ResponseFeedback> {
        // Try to find existing feedback
        const existing = await this.getUserFeedback(messageId, userId)

        if (existing) {
            // Update existing feedback
            const [updated] = await this.db
                .update(responseFeedback)
                .set({
                    feedbackType,
                    updatedAt: new Date(),
                })
                .where(
                    and(
                        eq(responseFeedback.messageId, messageId),
                        eq(responseFeedback.userId, userId),
                    ),
                )
                .returning()

            return updated
        } else {
            // Create new feedback
            const feedbackId = ulid()
            const [newFeedback] = await this.db
                .insert(responseFeedback)
                .values({
                    id: feedbackId,
                    messageId,
                    userId,
                    feedbackType,
                })
                .returning()

            return newFeedback
        }
    }

    /**
     * Get all feedback for a message
     */
    async getByMessageId(messageId: string): Promise<ResponseFeedback[]> {
        return await this.db
            .select()
            .from(responseFeedback)
            .where(eq(responseFeedback.messageId, messageId))
    }

    /**
     * Get specific user's feedback for a message
     */
    async getUserFeedback(messageId: string, userId: string): Promise<ResponseFeedback | null> {
        const [feedback] = await this.db
            .select()
            .from(responseFeedback)
            .where(
                and(eq(responseFeedback.messageId, messageId), eq(responseFeedback.userId, userId)),
            )
            .limit(1)

        return feedback || null
    }

    /**
     * Delete feedback for a message by a user
     */
    async delete(messageId: string, userId: string): Promise<boolean> {
        const result = await this.db
            .delete(responseFeedback)
            .where(
                and(eq(responseFeedback.messageId, messageId), eq(responseFeedback.userId, userId)),
            )

        return result.rowCount > 0
    }
}

// Export default instance for convenience
export const responseFeedbackRepository = new ResponseFeedbackRepository()
