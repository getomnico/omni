import { db } from '../db'
import { sql } from 'drizzle-orm'
import { ulid } from 'ulid'
import { UserOAuthCredentialsService } from './userCredentials'
import { isEmailFromApprovedDomain } from '../domains'
import type { OAuthProfile, OAuthTokens } from './types'

export interface User {
    id: string
    email: string
    name: string
    role: string
    created_at: Date
    updated_at: Date
}

export interface OAuthAuthResult {
    user: User
    isNewUser: boolean
    isLinkedAccount: boolean
}

export class AccountLinkingService {
    static async authenticateOrCreateUser(
        provider: string,
        profile: OAuthProfile,
        tokens: OAuthTokens,
    ): Promise<OAuthAuthResult> {
        // First, check if this OAuth account is already linked to a user
        const existingCredential = await UserOAuthCredentialsService.findByProviderProfile(
            provider,
            profile.id,
        )

        if (existingCredential) {
            // Update tokens and get the user
            await UserOAuthCredentialsService.updateTokens(
                existingCredential.user_id,
                provider,
                profile.id,
                tokens,
            )

            const user = await this.getUserById(existingCredential.user_id)
            return {
                user,
                isNewUser: false,
                isLinkedAccount: false,
            }
        }

        // Check if a user exists with this email
        const existingUser = await this.findUserByEmail(profile.email)

        if (existingUser) {
            // Link the OAuth account to the existing user
            await UserOAuthCredentialsService.saveCredentials(
                existingUser.id,
                provider,
                profile,
                tokens,
            )

            // Update user profile with OAuth data if needed
            await this.updateUserProfile(existingUser, profile)

            return {
                user: existingUser,
                isNewUser: false,
                isLinkedAccount: true,
            }
        }

        // Check if the email domain is approved for auto-registration
        const isDomainApproved = await isEmailFromApprovedDomain(profile.email)

        if (!isDomainApproved) {
            throw new Error(
                `Registration is not allowed for domain: ${profile.email.split('@')[1]}. ` +
                    'Please contact your administrator to approve your domain.',
            )
        }

        // Create new user with OAuth account
        const newUser = await this.createUserFromOAuth(profile)

        // Save OAuth credentials for the new user
        await UserOAuthCredentialsService.saveCredentials(newUser.id, provider, profile, tokens)

        return {
            user: newUser,
            isNewUser: true,
            isLinkedAccount: false,
        }
    }

    static async linkAccountToUser(
        userId: string,
        provider: string,
        profile: OAuthProfile,
        tokens: OAuthTokens,
    ): Promise<void> {
        // Check if this OAuth account is already linked to another user
        const existingCredential = await UserOAuthCredentialsService.findByProviderProfile(
            provider,
            profile.id,
        )

        if (existingCredential && existingCredential.user_id !== userId) {
            throw new Error(`This ${provider} account is already linked to another user.`)
        }

        // Get the user to verify email matches
        const user = await this.getUserById(userId)

        if (user.email !== profile.email) {
            throw new Error(
                'The email address of your OAuth account does not match your current account email.',
            )
        }

        // Save or update the OAuth credentials
        await UserOAuthCredentialsService.saveCredentials(userId, provider, profile, tokens)
    }

    private static async findUserByEmail(email: string): Promise<User | null> {
        const result = await db.execute(sql`
            SELECT * FROM users WHERE email = ${email} LIMIT 1
        `)

        if (!result.rows.length) {
            return null
        }

        const row = result.rows[0] as any
        return {
            id: row.id,
            email: row.email,
            name: row.name,
            role: row.role,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }

    private static async getUserById(id: string): Promise<User> {
        const result = await db.execute(sql`
            SELECT * FROM users WHERE id = ${id} LIMIT 1
        `)

        if (!result.rows.length) {
            throw new Error('User not found')
        }

        const row = result.rows[0] as any
        return {
            id: row.id,
            email: row.email,
            name: row.name,
            role: row.role,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }

    private static async createUserFromOAuth(profile: OAuthProfile): Promise<User> {
        const id = ulid()
        const name =
            profile.name ||
            `${profile.given_name || ''} ${profile.family_name || ''}`.trim() ||
            profile.email

        // Default role is 'user', first user gets 'admin' role
        const firstUserResult = await db.execute(sql`SELECT COUNT(*) as count FROM users`)
        const userCount = parseInt((firstUserResult.rows[0] as any).count)
        const role = userCount === 0 ? 'admin' : 'user'

        await db.execute(sql`
            INSERT INTO users (id, email, name, role, password_hash, email_verified)
            VALUES (${id}, ${profile.email}, ${name}, ${role}, '', ${profile.email_verified || false})
        `)

        return this.getUserById(id)
    }

    private static async updateUserProfile(user: User, profile: OAuthProfile): Promise<void> {
        // Update name if the OAuth profile has a more complete name
        const oauthName =
            profile.name || `${profile.given_name || ''} ${profile.family_name || ''}`.trim()

        if (oauthName && oauthName !== user.name && oauthName !== user.email) {
            await db.execute(sql`
                UPDATE users 
                SET name = ${oauthName}, updated_at = NOW()
                WHERE id = ${user.id}
            `)
        }

        // Update email verification status if OAuth provider verified the email
        if (profile.email_verified) {
            await db.execute(sql`
                UPDATE users 
                SET email_verified = true, updated_at = NOW()
                WHERE id = ${user.id} AND email_verified = false
            `)
        }
    }
}
