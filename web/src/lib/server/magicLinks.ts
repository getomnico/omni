import { eq, and, gt, isNull } from 'drizzle-orm'
import { db } from './db'
import { magicLinks, user, type MagicLink } from './db/schema'
import { createId } from '@paralleldrive/cuid2'
import { createHash, randomBytes } from 'crypto'

export interface MagicLinkResult {
    success: boolean
    magicLink?: {
        id: string
        token: string
        expiresAt: Date
    }
    error?: string
}

export interface MagicLinkVerificationResult {
    success: boolean
    user?: {
        id: string
        email: string
        role: string
        authMethod: string
        domain: string | null
    }
    error?: string
    requiresRegistration?: boolean
}

export class MagicLinkService {
    private static readonly TOKEN_LENGTH = 32
    private static readonly EXPIRY_MINUTES = 15

    static async createMagicLink(email: string, userId?: string): Promise<MagicLinkResult> {
        try {
            const token = randomBytes(this.TOKEN_LENGTH).toString('base64url')
            const tokenHash = createHash('sha256').update(token).digest('hex')

            const expiresAt = new Date()
            expiresAt.setMinutes(expiresAt.getMinutes() + this.EXPIRY_MINUTES)

            const id = createId()

            await db.insert(magicLinks).values({
                id,
                email,
                tokenHash,
                expiresAt,
                userId: userId || null,
            })

            return {
                success: true,
                magicLink: {
                    id,
                    token,
                    expiresAt,
                },
            }
        } catch (error) {
            console.error('Error creating magic link:', error)
            return {
                success: false,
                error: 'Failed to create magic link',
            }
        }
    }

    static async verifyMagicLink(token: string): Promise<MagicLinkVerificationResult> {
        try {
            const tokenHash = createHash('sha256').update(token).digest('hex')

            const magicLink = await db
                .select()
                .from(magicLinks)
                .where(
                    and(
                        eq(magicLinks.tokenHash, tokenHash),
                        gt(magicLinks.expiresAt, new Date()),
                        isNull(magicLinks.usedAt),
                    ),
                )
                .limit(1)

            if (magicLink.length === 0) {
                return {
                    success: false,
                    error: 'Invalid or expired magic link',
                }
            }

            const link = magicLink[0]

            // Mark the magic link as used
            await db
                .update(magicLinks)
                .set({ usedAt: new Date() })
                .where(eq(magicLinks.id, link.id))

            // Check if user already exists
            if (link.userId) {
                const existingUser = await db
                    .select()
                    .from(user)
                    .where(eq(user.id, link.userId))
                    .limit(1)

                if (existingUser.length > 0) {
                    return {
                        success: true,
                        user: {
                            id: existingUser[0].id,
                            email: existingUser[0].email,
                            role: existingUser[0].role,
                            authMethod: existingUser[0].authMethod,
                            domain: existingUser[0].domain,
                        },
                    }
                }
            }

            // Check if user with this email exists
            const existingUser = await db
                .select()
                .from(user)
                .where(eq(user.email, link.email))
                .limit(1)

            if (existingUser.length > 0) {
                return {
                    success: true,
                    user: {
                        id: existingUser[0].id,
                        email: existingUser[0].email,
                        role: existingUser[0].role,
                        authMethod: existingUser[0].authMethod,
                        domain: existingUser[0].domain,
                    },
                }
            }

            // User doesn't exist, check if they can auto-register
            return {
                success: true,
                requiresRegistration: true,
            }
        } catch (error) {
            console.error('Error verifying magic link:', error)
            return {
                success: false,
                error: 'Failed to verify magic link',
            }
        }
    }

    static async cleanupExpiredLinks(): Promise<void> {
        try {
            await db.delete(magicLinks).where(gt(new Date(), magicLinks.expiresAt))
        } catch (error) {
            console.error('Error cleaning up expired magic links:', error)
        }
    }

    static async revokeMagicLinksForUser(userId: string): Promise<void> {
        try {
            await db
                .update(magicLinks)
                .set({ usedAt: new Date() })
                .where(eq(magicLinks.userId, userId))
        } catch (error) {
            console.error('Error revoking magic links for user:', error)
        }
    }

    static async revokeMagicLinksForEmail(email: string): Promise<void> {
        try {
            await db
                .update(magicLinks)
                .set({ usedAt: new Date() })
                .where(eq(magicLinks.email, email))
        } catch (error) {
            console.error('Error revoking magic links for email:', error)
        }
    }

    static generateMagicLinkUrl(token: string, baseUrl: string): string {
        return `${baseUrl}/auth/magic-link?token=${encodeURIComponent(token)}`
    }
}
